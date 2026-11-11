//! Windows Search-backed file results for the launcher.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crossbeam_channel::{Receiver, Sender};

use super::{row, RowAction, SearchContribution, SearchSource};

const MAX_FILE_RESULTS: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileHit {
    pub name: String,
    pub path: PathBuf,
}

struct FileSearchOutcome {
    query: String,
    result: Result<Vec<FileHit>, String>,
}

pub(crate) struct FileSearchCache {
    service: FileSearchService,
    query: String,
    hits: Vec<FileHit>,
    failed: bool,
}

impl FileSearchCache {
    pub(crate) fn new() -> Self {
        Self {
            service: FileSearchService::new(),
            query: String::new(),
            hits: Vec::new(),
            failed: false,
        }
    }

    pub(crate) fn set_query(&mut self, query: &str) {
        if self.query == query {
            return;
        }

        self.query.clear();
        self.query.push_str(query);
        self.hits.clear();
        self.failed = false;

        if !query.is_empty() {
            self.service.request(query.to_owned());
        }
    }

    pub(crate) fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(outcome) = self.service.results.try_recv() {
            if outcome.query != self.query {
                continue;
            }

            match outcome.result {
                Ok(hits) => {
                    self.hits = hits;
                    self.failed = false;
                }
                Err(error) => {
                    tracing::warn!(error, "query Windows Search index");
                    self.hits.clear();
                    self.failed = true;
                }
            }
            changed = true;
        }
        changed
    }

    #[cfg(test)]
    fn with_results(query: &str, hits: Vec<FileHit>, failed: bool) -> Self {
        Self {
            service: FileSearchService::disabled(),
            query: query.to_owned(),
            hits,
            failed,
        }
    }
}

pub(crate) struct FileSearchSource<'a> {
    pub query: &'a str,
    pub cache: &'a FileSearchCache,
    pub icon_cache: &'a mut HashMap<PathBuf, Option<String>>,
    pub unavailable_title: &'a str,
    pub unavailable_subtitle: &'a str,
}

impl SearchSource for FileSearchSource<'_> {
    fn contribute(&mut self, query: &str) -> SearchContribution {
        if query.is_empty() || query != self.query {
            return SearchContribution::default();
        }

        let mut contribution = SearchContribution::default();
        if self.cache.failed {
            contribution.push(
                row(
                    "file-search:unavailable",
                    self.unavailable_title,
                    self.unavailable_subtitle,
                    file_icon(
                        self.icon_cache,
                        Path::new("%SystemRoot%\\ImmersiveControlPanel\\SystemSettings.exe"),
                    ),
                ),
                RowAction::OpenSettings("ms-settings:search".into()),
            );
            return contribution;
        }

        for hit in self.cache.hits.iter().take(MAX_FILE_RESULTS) {
            let subtitle = hit
                .path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(|parent| parent.to_string_lossy().into_owned())
                .unwrap_or_else(|| hit.path.to_string_lossy().into_owned());
            contribution.push(
                row(
                    format!("file:{}", hit.path.display()),
                    hit.name.clone(),
                    subtitle,
                    file_icon(self.icon_cache, &hit.path),
                )
                .with_subtitle_after_title(),
                RowAction::OpenFile(hit.path.clone()),
            );
        }
        contribution
    }
}

fn file_icon(icon_cache: &mut HashMap<PathBuf, Option<String>>, path: &Path) -> Option<String> {
    if let Some(icon) = icon_cache.get(path) {
        return icon.clone();
    }
    #[cfg(target_os = "windows")]
    let icon = crate::app_icons::app_icon_data_url(path);
    #[cfg(not(target_os = "windows"))]
    let icon = None;
    icon_cache.insert(path.to_path_buf(), icon.clone());
    icon
}

struct FileSearchService {
    requests: Option<Sender<String>>,
    results: Receiver<FileSearchOutcome>,
}

impl FileSearchService {
    fn new() -> Self {
        let (requests, request_rx) = crossbeam_channel::unbounded();
        let (result_tx, results) = crossbeam_channel::unbounded();
        std::thread::Builder::new()
            .name("steward-file-search".into())
            .spawn(move || run_worker(request_rx, result_tx))
            .expect("spawn Windows file search worker");
        Self {
            requests: Some(requests),
            results,
        }
    }

    fn request(&self, query: String) {
        if let Some(requests) = &self.requests {
            let _ = requests.send(query);
        }
    }

    #[cfg(test)]
    fn disabled() -> Self {
        let (_requests, request_rx) = crossbeam_channel::unbounded::<String>();
        let (_result_tx, results) = crossbeam_channel::unbounded::<FileSearchOutcome>();
        drop(request_rx);
        Self {
            requests: None,
            results,
        }
    }
}

impl Drop for FileSearchService {
    fn drop(&mut self) {
        self.requests.take();
    }
}

fn run_worker(requests: Receiver<String>, results: Sender<FileSearchOutcome>) {
    let mut backend = Backend::default();
    while let Ok(mut query) = requests.recv() {
        while let Ok(latest) = requests.try_recv() {
            query = latest;
        }
        let result = backend.search(&query);
        if results.send(FileSearchOutcome { query, result }).is_err() {
            return;
        }
    }
}

#[cfg(target_os = "windows")]
type Backend = windows_search::WindowsSearchBackend;

#[cfg(not(target_os = "windows"))]
#[derive(Default)]
struct Backend;

#[cfg(not(target_os = "windows"))]
impl Backend {
    fn search(&mut self, _query: &str) -> Result<Vec<FileHit>, String> {
        Err("Windows Search is only available on Windows".into())
    }
}

#[cfg(target_os = "windows")]
mod windows_search {
    use std::path::PathBuf;

    use windows::{
        core::{IUnknown, Interface, BSTR, GUID, PCWSTR},
        Win32::System::{
            Com::{
                CLSIDFromProgID, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
                IDispatch, CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED,
                DISPATCH_METHOD, DISPPARAMS,
            },
            Search::{CSearchManager, ISearchCatalogManager, ISearchManager, ISearchQueryHelper},
            Variant::VARIANT,
        },
    };

    use super::{FileHit, MAX_FILE_RESULTS};

    pub(super) const SEARCH_COLUMNS: &str = "System.ItemNameDisplay,System.ItemUrl";
    const CONNECTION_PROG_ID: &str = "ADODB.Connection";
    pub(super) const WHERE_RESTRICTIONS: &str =
        "AND System.FileName IS NOT NULL AND System.ItemUrl IS NOT NULL";

    #[derive(Default)]
    pub(super) struct WindowsSearchBackend {
        search: Option<WindowsSearch>,
    }

    impl WindowsSearchBackend {
        pub(super) fn search(&mut self, query: &str) -> Result<Vec<FileHit>, String> {
            if self.search.is_none() {
                self.search = Some(WindowsSearch::new().map_err(|error| format!("{error}"))?);
            }
            let result = self
                .search
                .as_mut()
                .expect("Windows Search initialized")
                .search(query);
            if result.is_err() {
                self.search = None;
            }
            result
        }
    }

    struct WindowsSearch {
        _com: ComApartment,
        helper: ISearchQueryHelper,
        connection: IDispatch,
    }

    impl WindowsSearch {
        fn new() -> windows::core::Result<Self> {
            let com = ComApartment::new()?;
            let manager: ISearchManager =
                unsafe { CoCreateInstance(&CSearchManager, None, CLSCTX_LOCAL_SERVER) }?;
            let catalog_name = windows_string("SystemIndex");
            let catalog: ISearchCatalogManager =
                unsafe { manager.GetCatalog(catalog_name.as_pcwstr()) }?;
            let helper: ISearchQueryHelper = unsafe { catalog.GetQueryHelper() }?;

            let columns = windows_string(SEARCH_COLUMNS);
            unsafe {
                helper.SetQuerySelectColumns(columns.as_pcwstr())?;
                helper.SetQueryMaxResults(MAX_FILE_RESULTS as i32)?;
            }

            let restrictions = windows_string(WHERE_RESTRICTIONS);
            unsafe { helper.SetQueryWhereRestrictions(restrictions.as_pcwstr())? };

            let connection_string = unsafe { take_task_string(helper.ConnectionString()?) }?;
            let class_id = unsafe {
                let prog_id = windows_string(CONNECTION_PROG_ID);
                CLSIDFromProgID(prog_id.as_pcwstr())?
            };
            let unknown: IUnknown =
                unsafe { CoCreateInstance(&class_id, None, CLSCTX_INPROC_SERVER) }?;
            let connection: IDispatch = unknown.cast()?;
            invoke(
                &connection,
                "Open",
                vec![connection_string.as_str().into()],
                DISPATCH_METHOD,
            )?;

            Ok(Self {
                _com: com,
                helper,
                connection,
            })
        }

        fn search(&mut self, query: &str) -> Result<Vec<FileHit>, String> {
            unsafe { self.search_inner(query) }.map_err(|error| format!("{error}"))
        }

        unsafe fn search_inner(&self, query: &str) -> windows::core::Result<Vec<FileHit>> {
            let query = windows_string(query);
            let sql = unsafe {
                take_task_string(self.helper.GenerateSQLFromUserQuery(query.as_pcwstr())?)
            }?;
            let result = invoke(
                &self.connection,
                "Execute",
                vec![sql.as_str().into()],
                DISPATCH_METHOD,
            )?;
            let recordset: IDispatch = (&result).try_into()?;
            let result = invoke(
                &recordset,
                "GetString",
                vec![
                    2_i32.into(),
                    (-1_i32).into(),
                    "\t".into(),
                    "\n".into(),
                    "".into(),
                ],
                DISPATCH_METHOD,
            )?;
            let text: BSTR = (&result).try_into()?;
            Ok(parse_rows(&text.to_string()))
        }
    }

    pub(super) fn parse_rows(text: &str) -> Vec<FileHit> {
        text.lines()
            .filter_map(|line| {
                let line = line.strip_suffix('\r').unwrap_or(line);
                let (name, item_url) = line.split_once('\t')?;
                let path = path_from_item_url(item_url)?;
                let name = if name.is_empty() {
                    path.file_name()?.to_string_lossy().into_owned()
                } else {
                    name.to_owned()
                };
                Some(FileHit { name, path })
            })
            .collect()
    }

    pub(super) fn path_from_item_url(item_url: &str) -> Option<PathBuf> {
        let path = item_url.strip_prefix("file:")?;
        if path.is_empty() {
            return None;
        }

        // Search returns canonical item URLs. Depending on the provider, the
        // file URL can be `file:C:\...`, `file:///C:/...`, or a UNC URL.
        let path = path
            .strip_prefix("//localhost/")
            .or_else(|| path.strip_prefix("//LOCALHOST/"))
            .unwrap_or(path);
        let normalized = if path.starts_with("//") {
            format!("file:{path}")
        } else {
            format!(
                "file:///{}",
                path.trim_start_matches('/').replace('\\', "/")
            )
        };
        let path = url::Url::parse(&normalized).ok()?.to_file_path().ok()?;
        path.is_absolute().then_some(path)
    }

    fn invoke(
        dispatch: &IDispatch,
        name: &str,
        args: Vec<VARIANT>,
        flags: windows::Win32::System::Com::DISPATCH_FLAGS,
    ) -> windows::core::Result<VARIANT> {
        let name = windows_string(name);
        let names = [name.as_pcwstr()];
        let mut member = 0;
        unsafe {
            dispatch.GetIDsOfNames(&GUID::zeroed(), names.as_ptr(), 1, 0, &mut member)?;
        }

        let mut args = args;
        args.reverse();
        let params = DISPPARAMS {
            rgvarg: args.as_mut_ptr(),
            rgdispidNamedArgs: std::ptr::null_mut(),
            cArgs: args.len() as u32,
            cNamedArgs: 0,
        };
        let mut result = VARIANT::default();
        unsafe {
            dispatch.Invoke(
                member,
                &GUID::zeroed(),
                0,
                flags,
                &params,
                Some(&mut result),
                None,
                None,
            )?;
        }
        Ok(result)
    }

    struct ComApartment;

    impl ComApartment {
        fn new() -> windows::core::Result<Self> {
            unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok()?;
            Ok(Self)
        }
    }

    impl Drop for ComApartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    struct WindowsString(Vec<u16>);

    impl WindowsString {
        fn as_pcwstr(&self) -> PCWSTR {
            PCWSTR(self.0.as_ptr())
        }
    }

    fn windows_string(value: &str) -> WindowsString {
        WindowsString(value.encode_utf16().chain(std::iter::once(0)).collect())
    }

    unsafe fn take_task_string(value: windows::core::PWSTR) -> windows::core::Result<String> {
        if value.is_null() {
            return Err(windows::core::Error::from_win32());
        }
        let result = unsafe { value.to_string() }.map_err(|_| windows::core::Error::from_win32());
        unsafe { CoTaskMemFree(Some(value.0.cast())) };
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(path: &str) -> FileHit {
        FileHit {
            name: Path::new(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            path: PathBuf::from(path),
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn file_results_become_launch_actions_with_parent_subtitles() {
        let cache = FileSearchCache::with_results(
            "report",
            vec![hit(r"C:\Users\test\Documents\report.pdf")],
            false,
        );
        let mut icons = HashMap::new();
        icons.insert(PathBuf::from(r"C:\Users\test\Documents\report.pdf"), None);
        let mut source = FileSearchSource {
            query: "report",
            cache: &cache,
            icon_cache: &mut icons,
            unavailable_title: "Unavailable",
            unavailable_subtitle: "Open settings",
        };

        let contribution = source.contribute("report");
        assert_eq!(contribution.rows[0].title, "report.pdf");
        assert_eq!(contribution.rows[0].subtitle, r"C:\Users\test\Documents");
        assert_eq!(
            contribution.rows[0].subtitle_placement,
            steward_ui_components::SubtitlePlacement::AfterTitle
        );
        assert!(matches!(
            &contribution.actions[0],
            RowAction::OpenFile(path)
                if path == Path::new(r"C:\Users\test\Documents\report.pdf")
        ));
    }

    #[test]
    fn failed_search_offers_the_search_settings_repair_path() {
        let cache = FileSearchCache::with_results("report", vec![], true);
        let mut icons = HashMap::new();
        icons.insert(
            PathBuf::from("%SystemRoot%\\ImmersiveControlPanel\\SystemSettings.exe"),
            None,
        );
        let mut source = FileSearchSource {
            query: "report",
            cache: &cache,
            icon_cache: &mut icons,
            unavailable_title: "Unavailable",
            unavailable_subtitle: "Open settings",
        };

        let contribution = source.contribute("report");
        assert_eq!(contribution.rows[0].title, "Unavailable");
        assert!(matches!(
            &contribution.actions[0],
            RowAction::OpenSettings(uri) if uri == "ms-settings:search"
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_search_is_not_limited_to_a_path_scope() {
        assert_eq!(
            windows_search::WHERE_RESTRICTIONS,
            "AND System.FileName IS NOT NULL AND System.ItemUrl IS NOT NULL"
        );
        assert!(!windows_search::WHERE_RESTRICTIONS.contains("Scope"));
        assert_eq!(
            windows_search::SEARCH_COLUMNS,
            "System.ItemNameDisplay,System.ItemUrl"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn search_item_urls_become_canonical_file_paths() {
        assert_eq!(
            windows_search::path_from_item_url(r"file:C:\Users\test\Documents\report.pdf"),
            Some(PathBuf::from(r"C:\Users\test\Documents\report.pdf"))
        );
        assert_eq!(
            windows_search::path_from_item_url("file:///C:/Users/test/My%20Documents/report.pdf"),
            Some(PathBuf::from(r"C:\Users\test\My Documents\report.pdf"))
        );
        assert_eq!(
            windows_search::path_from_item_url("file://localhost/C:/Users/test/report.pdf"),
            Some(PathBuf::from(r"C:\Users\test\report.pdf"))
        );
        assert_eq!(windows_search::path_from_item_url("search://item"), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn search_rows_use_the_item_url_not_the_localized_display_path() {
        let rows =
            windows_search::parse_rows("report.pdf\tfile:C:\\Users\\test\\Documents\\report.pdf");
        assert_eq!(
            rows,
            vec![FileHit {
                name: "report.pdf".into(),
                path: PathBuf::from(r"C:\Users\test\Documents\report.pdf"),
            }]
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "queries the host Windows Search index"]
    fn live_windows_search_index_returns_file_paths() {
        let mut backend = windows_search::WindowsSearchBackend::default();
        let hits = backend
            .search("readme")
            .expect("query the Windows Search index");
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|hit| hit.path.is_absolute()));
        assert!(hits.iter().all(|hit| hit.path.exists()));
    }
}
