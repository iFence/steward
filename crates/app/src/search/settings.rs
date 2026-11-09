use std::cell::RefCell;

use steward_core_engine::nucleo::{Config, Matcher, Utf32Str, Utf32String};
use steward_core_engine::search_haystacks;

use super::{row, RowAction, SearchContribution, SearchSource};

const MAX_SETTINGS_RESULTS: usize = 16;
const EXACT_TITLE_BOOST: u32 = 4_000;
const EXACT_ALIAS_BOOST: u32 = 3_000;
const PREFIX_TITLE_BOOST: u32 = 1_200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SettingsEntry {
    uri: &'static str,
    title_en: &'static str,
    title_zh: &'static str,
    aliases: &'static [&'static str],
}

impl SettingsEntry {
    fn title(self, locale: &str) -> &'static str {
        if locale.to_ascii_lowercase().starts_with("zh") {
            self.title_zh
        } else {
            self.title_en
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SettingsHit<'a> {
    entry: &'a SettingsEntry,
    score: u32,
}

/// Prebuilt index of Windows Settings pages and their localized search terms.
///
/// The catalog is static because the `ms-settings:` URI scheme is stable and
/// opening a page must not depend on a background scan or a Windows Search
/// service. Each title and alias still gets the same pinyin haystacks as an
/// application, so settings remain searchable from a Chinese UI.
pub(crate) struct SettingsIndex {
    entries: Vec<SettingsEntry>,
    haystacks: Vec<Vec<Utf32String>>,
    matcher: RefCell<Matcher>,
}

impl SettingsIndex {
    pub(crate) fn new() -> Self {
        let entries = SETTINGS.to_vec();
        let haystacks = entries
            .iter()
            .map(|entry| {
                let mut haystacks = Vec::new();
                for text in [entry.title_en, entry.title_zh]
                    .into_iter()
                    .chain(entry.aliases.iter().copied())
                {
                    haystacks.extend(search_haystacks(text));
                }
                if let Some(uri_name) = entry.uri.strip_prefix("ms-settings:") {
                    haystacks.extend(search_haystacks(uri_name));
                }
                haystacks
            })
            .collect();
        Self {
            entries,
            haystacks,
            matcher: RefCell::new(Matcher::new(Config::DEFAULT)),
        }
    }

    fn query(&self, query: &str) -> Vec<SettingsHit<'_>> {
        let folded = query.trim().to_lowercase();
        if folded.is_empty() {
            return Vec::new();
        }

        let mut matcher = self.matcher.borrow_mut();
        let mut needle_buf = Vec::new();
        let needle = Utf32Str::new(&folded, &mut needle_buf);
        let mut hits = Vec::new();
        for (entry, haystacks) in self.entries.iter().zip(&self.haystacks) {
            let mut best = None;
            for haystack in haystacks {
                let haystack: Utf32Str<'_> = haystack.slice(..);
                if let Some(score) = matcher.fuzzy_match(haystack, needle) {
                    best = Some(best.map_or(score, |current: u16| current.max(score)));
                }
            }
            if let Some(score) = best {
                hits.push(SettingsHit {
                    entry,
                    score: score as u32 + relevance_boost(entry, &folded),
                });
            }
        }
        hits.sort_by_key(|hit| std::cmp::Reverse(hit.score));
        hits.truncate(MAX_SETTINGS_RESULTS);
        hits
    }
}

impl Default for SettingsIndex {
    fn default() -> Self {
        Self::new()
    }
}

fn relevance_boost(entry: &SettingsEntry, query: &str) -> u32 {
    let title_matches = |title: &str| {
        if title.eq_ignore_ascii_case(query) {
            EXACT_TITLE_BOOST
        } else if title.to_lowercase().starts_with(query) {
            PREFIX_TITLE_BOOST
        } else {
            0
        }
    };
    let title_boost = title_matches(entry.title_en).max(title_matches(entry.title_zh));
    let alias_boost = if entry
        .aliases
        .iter()
        .any(|alias| alias.eq_ignore_ascii_case(query))
    {
        EXACT_ALIAS_BOOST
    } else {
        0
    };
    title_boost.max(alias_boost)
}

pub(crate) struct SettingsSource<'a> {
    pub index: &'a SettingsIndex,
    pub label: &'a str,
    pub locale: &'a str,
}

impl SearchSource for SettingsSource<'_> {
    fn contribute(&mut self, query: &str) -> SearchContribution {
        self.contribute_with_icon(query, settings_icon())
    }
}

impl SettingsSource<'_> {
    fn contribute_with_icon(&mut self, query: &str, icon: Option<String>) -> SearchContribution {
        let mut contribution = SearchContribution::default();
        for hit in self.index.query(query) {
            let entry = hit.entry;
            contribution.push(
                row(
                    format!("settings:{}", entry.uri),
                    entry.title(self.locale),
                    self.label,
                    icon.clone(),
                ),
                RowAction::OpenSettings(entry.uri.into()),
            );
        }
        contribution
    }
}

fn settings_icon() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        crate::app_icons::settings_icon_data_url()
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

const SETTINGS: &[SettingsEntry] = &[
    SettingsEntry {
        uri: "ms-settings:",
        title_en: "System Settings",
        title_zh: "系统设置",
        aliases: &[
            "settings",
            "windows settings",
            "system settings",
            "设置",
            "windows 设置",
            "系统设置",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:display",
        title_en: "Display",
        title_zh: "显示",
        aliases: &[
            "screen",
            "monitor",
            "brightness",
            "resolution",
            "display settings",
            "显示器",
            "屏幕",
            "亮度",
            "分辨率",
            "显示设置",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:nightlight",
        title_en: "Night Light",
        title_zh: "夜间模式",
        aliases: &[
            "night mode",
            "blue light",
            "warm color",
            "护眼",
            "夜间模式设置",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:display-advanced",
        title_en: "Advanced Display",
        title_zh: "高级显示器设置",
        aliases: &["refresh rate", "hdr", "color profile", "刷新率", "高级显示"],
    },
    SettingsEntry {
        uri: "ms-settings:display-advancedgraphics",
        title_en: "Graphics",
        title_zh: "图形",
        aliases: &["gpu", "graphics settings", "显卡", "gpu 设置"],
    },
    SettingsEntry {
        uri: "ms-settings:sound",
        title_en: "Sound",
        title_zh: "声音",
        aliases: &[
            "audio",
            "volume",
            "speakers",
            "microphone",
            "sound settings",
            "音频",
            "音量",
            "扬声器",
            "声音设置",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:notifications",
        title_en: "Notifications",
        title_zh: "通知",
        aliases: &[
            "alerts",
            "banners",
            "notification settings",
            "提醒",
            "通知设置",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:quiethours",
        title_en: "Focus",
        title_zh: "专注",
        aliases: &[
            "focus assist",
            "do not disturb",
            "quiet hours",
            "免打扰",
            "专注助手",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:powersleep",
        title_en: "Power & Sleep",
        title_zh: "电源和睡眠",
        aliases: &[
            "power",
            "sleep",
            "screen timeout",
            "电源",
            "睡眠",
            "休眠",
            "屏幕超时",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:batterysaver",
        title_en: "Battery Saver",
        title_zh: "节电模式",
        aliases: &["battery", "battery settings", "电池", "省电", "节电"],
    },
    SettingsEntry {
        uri: "ms-settings:storagesense",
        title_en: "Storage",
        title_zh: "存储",
        aliases: &[
            "disk space",
            "storage sense",
            "cleanup",
            "磁盘",
            "空间",
            "存储感知",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:nearby-share",
        title_en: "Nearby Sharing",
        title_zh: "就近共享",
        aliases: &["share files", "附近共享", "共享文件"],
    },
    SettingsEntry {
        uri: "ms-settings:multitasking",
        title_en: "Multitasking",
        title_zh: "多任务处理",
        aliases: &[
            "snap windows",
            "alt tab",
            "virtual desktops",
            "多任务",
            "贴靠窗口",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:project",
        title_en: "Projecting to This PC",
        title_zh: "投影到此电脑",
        aliases: &["project", "wireless display", "投影", "无线显示"],
    },
    SettingsEntry {
        uri: "ms-settings:crossdevice",
        title_en: "Shared Experiences",
        title_zh: "共享体验",
        aliases: &["shared experiences", "cross device", "跨设备", "共享"],
    },
    SettingsEntry {
        uri: "ms-settings:clipboard",
        title_en: "Clipboard",
        title_zh: "剪贴板",
        aliases: &["clipboard history", "copy paste", "剪贴板历史", "复制粘贴"],
    },
    SettingsEntry {
        uri: "ms-settings:remotedesktop",
        title_en: "Remote Desktop",
        title_zh: "远程桌面",
        aliases: &["rdp", "remote connection", "远程连接", "远程"],
    },
    SettingsEntry {
        uri: "ms-settings:optionalfeatures",
        title_en: "Optional Features",
        title_zh: "可选功能",
        aliases: &[
            "windows features",
            "add features",
            "windows 功能",
            "添加功能",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:appsfeatures",
        title_en: "Installed Apps",
        title_zh: "已安装的应用",
        aliases: &[
            "apps",
            "uninstall",
            "installed applications",
            "应用",
            "卸载",
            "安装的应用",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:startupapps",
        title_en: "Startup Apps",
        title_zh: "启动应用",
        aliases: &["startup", "boot apps", "开机启动", "启动项"],
    },
    SettingsEntry {
        uri: "ms-settings:defaultapps",
        title_en: "Default Apps",
        title_zh: "默认应用",
        aliases: &[
            "default programs",
            "file associations",
            "默认程序",
            "文件关联",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:appsforwebsites",
        title_en: "Apps for Websites",
        title_zh: "网站应用",
        aliases: &["web apps", "website apps", "网页应用"],
    },
    SettingsEntry {
        uri: "ms-settings:maps",
        title_en: "Offline Maps",
        title_zh: "离线地图",
        aliases: &["maps", "download maps", "地图", "下载地图"],
    },
    SettingsEntry {
        uri: "ms-settings:yourinfo",
        title_en: "Your Info",
        title_zh: "你的信息",
        aliases: &[
            "account",
            "profile",
            "microsoft account",
            "账户",
            "个人信息",
            "微软账户",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:emailandaccounts",
        title_en: "Email & Accounts",
        title_zh: "电子邮件和账户",
        aliases: &["email", "accounts", "邮箱", "账户"],
    },
    SettingsEntry {
        uri: "ms-settings:signinoptions",
        title_en: "Sign-in Options",
        title_zh: "登录选项",
        aliases: &[
            "password",
            "pin",
            "face",
            "fingerprint",
            "windows hello",
            "密码",
            "人脸识别",
            "指纹",
            "登录",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:otherusers",
        title_en: "Family & Other Users",
        title_zh: "家庭和其他用户",
        aliases: &[
            "users",
            "add user",
            "local account",
            "用户",
            "添加用户",
            "家庭",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:family-group",
        title_en: "Family Group",
        title_zh: "家庭组",
        aliases: &["family safety", "parental controls", "家庭安全", "家长控制"],
    },
    SettingsEntry {
        uri: "ms-settings:workplace",
        title_en: "Access Work or School",
        title_zh: "访问工作或学校",
        aliases: &["work account", "school account", "工作账户", "学校账户"],
    },
    SettingsEntry {
        uri: "ms-settings:sync",
        title_en: "Sync Your Settings",
        title_zh: "同步你的设置",
        aliases: &["sync settings", "roaming", "同步设置", "漫游"],
    },
    SettingsEntry {
        uri: "ms-settings:windowsupdate",
        title_en: "Windows Update",
        title_zh: "Windows 更新",
        aliases: &["update", "updates", "patch", "更新", "系统更新", "补丁"],
    },
    SettingsEntry {
        uri: "ms-settings:windowsupdate-history",
        title_en: "Update History",
        title_zh: "更新历史记录",
        aliases: &[
            "installed updates",
            "patch history",
            "更新历史",
            "已安装更新",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:windowsupdate-options",
        title_en: "Advanced Update Options",
        title_zh: "高级更新选项",
        aliases: &["pause updates", "update settings", "暂停更新", "更新选项"],
    },
    SettingsEntry {
        uri: "ms-settings:windowsupdate-activehours",
        title_en: "Active Hours",
        title_zh: "活动时间",
        aliases: &["restart hours", "working hours", "重启时间", "工作时间"],
    },
    SettingsEntry {
        uri: "ms-settings:windowsupdate-optionalupdates",
        title_en: "Optional Updates",
        title_zh: "可选更新",
        aliases: &[
            "driver updates",
            "optional drivers",
            "驱动程序更新",
            "可选更新",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:windowsinsider",
        title_en: "Windows Insider Program",
        title_zh: "Windows 预览体验计划",
        aliases: &["insider", "preview builds", "beta", "预览体验", "测试版"],
    },
    SettingsEntry {
        uri: "ms-settings:troubleshoot",
        title_en: "Troubleshoot",
        title_zh: "疑难解答",
        aliases: &["troubleshooter", "fix problems", "诊断", "修复问题"],
    },
    SettingsEntry {
        uri: "ms-settings:recovery",
        title_en: "Recovery",
        title_zh: "恢复",
        aliases: &[
            "reset pc",
            "advanced startup",
            "reset windows",
            "重置电脑",
            "恢复选项",
            "高级启动",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:activation",
        title_en: "Activation",
        title_zh: "激活",
        aliases: &[
            "windows activation",
            "product key",
            "windows 激活",
            "产品密钥",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:findmydevice",
        title_en: "Find My Device",
        title_zh: "查找我的设备",
        aliases: &["device location", "find device", "设备定位", "查找设备"],
    },
    SettingsEntry {
        uri: "ms-settings:developers",
        title_en: "For Developers",
        title_zh: "开发者选项",
        aliases: &[
            "developer mode",
            "developer settings",
            "开发者模式",
            "开发人员",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:windowsdefender",
        title_en: "Windows Security",
        title_zh: "Windows 安全中心",
        aliases: &[
            "defender",
            "antivirus",
            "firewall",
            "security",
            "安全",
            "杀毒",
            "防火墙",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:deviceencryption",
        title_en: "Device Encryption",
        title_zh: "设备加密",
        aliases: &["bitlocker", "encryption", "加密", "磁盘加密"],
    },
    SettingsEntry {
        uri: "ms-settings:about",
        title_en: "About",
        title_zh: "关于",
        aliases: &[
            "system info",
            "device info",
            "rename pc",
            "windows version",
            "系统信息",
            "设备信息",
            "重命名电脑",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:network",
        title_en: "Network & Internet",
        title_zh: "网络和 Internet",
        aliases: &[
            "network",
            "internet",
            "network settings",
            "网络",
            "互联网",
            "网络设置",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:network-status",
        title_en: "Network Status",
        title_zh: "网络状态",
        aliases: &["connection status", "network status", "连接状态"],
    },
    SettingsEntry {
        uri: "ms-settings:network-wifi",
        title_en: "Wi-Fi",
        title_zh: "Wi-Fi",
        aliases: &[
            "wifi",
            "wi fi",
            "wireless",
            "wlan",
            "无线网络",
            "无线",
            "wifi 设置",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:network-ethernet",
        title_en: "Ethernet",
        title_zh: "以太网",
        aliases: &["lan", "wired network", "有线网络", "有线"],
    },
    SettingsEntry {
        uri: "ms-settings:network-vpn",
        title_en: "VPN",
        title_zh: "VPN",
        aliases: &["virtual private network", "vpn settings", "虚拟专用网络"],
    },
    SettingsEntry {
        uri: "ms-settings:network-mobilehotspot",
        title_en: "Mobile Hotspot",
        title_zh: "移动热点",
        aliases: &[
            "hotspot",
            "tethering",
            "shared internet",
            "热点",
            "共享网络",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:network-airplanemode",
        title_en: "Airplane Mode",
        title_zh: "飞行模式",
        aliases: &["flight mode", "wireless off", "飞行", "无线关闭"],
    },
    SettingsEntry {
        uri: "ms-settings:network-proxy",
        title_en: "Proxy",
        title_zh: "代理",
        aliases: &["proxy server", "代理服务器", "网络代理"],
    },
    SettingsEntry {
        uri: "ms-settings:datausage",
        title_en: "Data Usage",
        title_zh: "数据使用量",
        aliases: &["data usage", "bandwidth", "流量", "数据用量"],
    },
    SettingsEntry {
        uri: "ms-settings:network-advancedsettings",
        title_en: "Advanced Network Settings",
        title_zh: "高级网络设置",
        aliases: &[
            "adapters",
            "network adapters",
            "dns",
            "网络适配器",
            "高级网络",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:bluetooth",
        title_en: "Bluetooth & Devices",
        title_zh: "蓝牙和其他设备",
        aliases: &[
            "bluetooth",
            "bt",
            "pair device",
            "wireless devices",
            "蓝牙",
            "设备",
            "配对设备",
            "蓝牙设置",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:printers",
        title_en: "Printers & Scanners",
        title_zh: "打印机和扫描仪",
        aliases: &["printer", "scanner", "print", "打印", "打印机", "扫描仪"],
    },
    SettingsEntry {
        uri: "ms-settings:mouse",
        title_en: "Mouse",
        title_zh: "鼠标",
        aliases: &[
            "pointer speed",
            "scroll",
            "mouse settings",
            "鼠标设置",
            "指针速度",
            "滚轮",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:devices-touchpad",
        title_en: "Touchpad",
        title_zh: "触摸板",
        aliases: &["trackpad", "touch pad", "触控板", "触摸板设置"],
    },
    SettingsEntry {
        uri: "ms-settings:pen",
        title_en: "Pen & Windows Ink",
        title_zh: "笔和 Windows Ink",
        aliases: &["stylus", "pen", "handwriting", "手写笔", "触控笔", "手写"],
    },
    SettingsEntry {
        uri: "ms-settings:autoplay",
        title_en: "AutoPlay",
        title_zh: "自动播放",
        aliases: &["autoplay", "removable drives", "自动运行", "可移动设备"],
    },
    SettingsEntry {
        uri: "ms-settings:usb",
        title_en: "USB",
        title_zh: "USB",
        aliases: &["usb settings", "usb devices", "usb 设备"],
    },
    SettingsEntry {
        uri: "ms-settings:mobile-devices",
        title_en: "Phone & Mobile Devices",
        title_zh: "手机和移动设备",
        aliases: &[
            "phone link",
            "mobile",
            "phone",
            "手机",
            "移动设备",
            "手机连接",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:personalization",
        title_en: "Personalization",
        title_zh: "个性化",
        aliases: &[
            "appearance",
            "desktop",
            "look and feel",
            "外观",
            "桌面",
            "主题",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:personalization-background",
        title_en: "Background",
        title_zh: "背景",
        aliases: &["wallpaper", "desktop background", "壁纸", "桌面背景"],
    },
    SettingsEntry {
        uri: "ms-settings:personalization-colors",
        title_en: "Colors",
        title_zh: "颜色",
        aliases: &[
            "dark mode",
            "accent color",
            "transparency",
            "深色模式",
            "主题色",
            "透明效果",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:themes",
        title_en: "Themes",
        title_zh: "主题",
        aliases: &[
            "theme",
            "wallpaper",
            "desktop icons",
            "壁纸",
            "桌面图标",
            "主题设置",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:lockscreen",
        title_en: "Lock Screen",
        title_zh: "锁屏界面",
        aliases: &["lock screen", "login screen", "锁屏", "登录屏幕"],
    },
    SettingsEntry {
        uri: "ms-settings:personalization-touchkeyboard",
        title_en: "Touch Keyboard",
        title_zh: "触摸键盘",
        aliases: &[
            "on screen keyboard",
            "virtual keyboard",
            "软键盘",
            "屏幕键盘",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:personalization-start",
        title_en: "Start",
        title_zh: "开始",
        aliases: &["start menu", "start screen", "开始菜单", "开始屏幕"],
    },
    SettingsEntry {
        uri: "ms-settings:personalization-taskbar",
        title_en: "Taskbar",
        title_zh: "任务栏",
        aliases: &[
            "taskbar",
            "system tray",
            "taskbar settings",
            "任务栏设置",
            "系统托盘",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:fonts",
        title_en: "Fonts",
        title_zh: "字体",
        aliases: &["font", "text", "字体设置", "文字"],
    },
    SettingsEntry {
        uri: "ms-settings:dateandtime",
        title_en: "Date & Time",
        title_zh: "日期和时间",
        aliases: &["clock", "time zone", "time", "日期", "时间", "时区", "时钟"],
    },
    SettingsEntry {
        uri: "ms-settings:regionlanguage",
        title_en: "Language & Region",
        title_zh: "语言和区域",
        aliases: &["language", "region", "locale", "区域", "国家", "语言设置"],
    },
    SettingsEntry {
        uri: "ms-settings:typing",
        title_en: "Typing",
        title_zh: "输入",
        aliases: &[
            "keyboard",
            "autocorrect",
            "spell check",
            "键盘",
            "输入法",
            "自动更正",
            "拼写检查",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:speech",
        title_en: "Speech",
        title_zh: "语音",
        aliases: &[
            "voice",
            "speech recognition",
            "text to speech",
            "语音识别",
            "文字转语音",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:gaming-gamebar",
        title_en: "Game Bar",
        title_zh: "游戏栏",
        aliases: &["xbox game bar", "overlay", "游戏覆盖", "游戏设置"],
    },
    SettingsEntry {
        uri: "ms-settings:gaming-gamedvr",
        title_en: "Captures",
        title_zh: "捕获",
        aliases: &[
            "game capture",
            "record gameplay",
            "screen recording",
            "游戏录制",
            "录屏",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:gaming-gamemode",
        title_en: "Game Mode",
        title_zh: "游戏模式",
        aliases: &["game mode", "gaming performance", "游戏性能"],
    },
    SettingsEntry {
        uri: "ms-settings:easeofaccess",
        title_en: "Accessibility",
        title_zh: "辅助功能",
        aliases: &["ease of access", "accessibility", "无障碍", "轻松使用"],
    },
    SettingsEntry {
        uri: "ms-settings:easeofaccess-display",
        title_en: "Text Size & Visual Effects",
        title_zh: "文本大小和视觉效果",
        aliases: &[
            "text size",
            "visual effects",
            "animations",
            "文本大小",
            "动画",
            "视觉效果",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:easeofaccess-magnifier",
        title_en: "Magnifier",
        title_zh: "放大镜",
        aliases: &["zoom", "screen magnifier", "缩放", "放大"],
    },
    SettingsEntry {
        uri: "ms-settings:easeofaccess-colorfilter",
        title_en: "Color Filters",
        title_zh: "颜色滤镜",
        aliases: &["color blind", "color filter", "色盲", "色彩滤镜"],
    },
    SettingsEntry {
        uri: "ms-settings:easeofaccess-highcontrast",
        title_en: "Contrast Themes",
        title_zh: "对比度主题",
        aliases: &["high contrast", "contrast theme", "高对比度", "对比度"],
    },
    SettingsEntry {
        uri: "ms-settings:easeofaccess-narrator",
        title_en: "Narrator",
        title_zh: "讲述人",
        aliases: &["screen reader", "read aloud", "屏幕阅读器", "朗读"],
    },
    SettingsEntry {
        uri: "ms-settings:easeofaccess-audio",
        title_en: "Audio Accessibility",
        title_zh: "音频辅助功能",
        aliases: &["mono audio", "hearing", "mono", "单声道", "听觉"],
    },
    SettingsEntry {
        uri: "ms-settings:easeofaccess-closedcaptioning",
        title_en: "Captions",
        title_zh: "字幕",
        aliases: &["closed captions", "subtitles", "隐藏式字幕", "字幕设置"],
    },
    SettingsEntry {
        uri: "ms-settings:easeofaccess-speechrecognition",
        title_en: "Voice Access",
        title_zh: "语音访问",
        aliases: &[
            "speech recognition",
            "voice control",
            "语音识别",
            "语音控制",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:easeofaccess-keyboard",
        title_en: "Keyboard Accessibility",
        title_zh: "键盘辅助功能",
        aliases: &[
            "sticky keys",
            "filter keys",
            "on screen keyboard",
            "粘滞键",
            "筛选键",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:easeofaccess-cursorandpointer",
        title_en: "Mouse Pointer & Touch",
        title_zh: "鼠标指针和触控",
        aliases: &[
            "cursor",
            "pointer size",
            "pointer color",
            "鼠标指针",
            "光标",
            "指针大小",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:search",
        title_en: "Search",
        title_zh: "搜索",
        aliases: &[
            "windows search",
            "search settings",
            "windows 搜索",
            "搜索设置",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:search-permissions",
        title_en: "Search Permissions",
        title_zh: "搜索权限",
        aliases: &["search indexing", "indexing options", "搜索索引", "索引"],
    },
    SettingsEntry {
        uri: "ms-settings:search-highlights",
        title_en: "Search Highlights",
        title_zh: "搜索亮点",
        aliases: &["search highlights", "search suggestions", "搜索建议"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy",
        title_en: "Privacy & Security",
        title_zh: "隐私和安全性",
        aliases: &["privacy", "permissions", "隐私", "权限", "安全"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-location",
        title_en: "Location",
        title_zh: "位置",
        aliases: &["location services", "gps", "定位", "位置服务"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-camera",
        title_en: "Camera",
        title_zh: "相机",
        aliases: &["camera access", "webcam", "摄像头", "相机权限"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-microphone",
        title_en: "Microphone",
        title_zh: "麦克风",
        aliases: &["mic", "microphone access", "话筒", "麦克风权限"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-voiceactivation",
        title_en: "Voice Activation",
        title_zh: "语音激活",
        aliases: &["voice activation", "voice commands", "语音唤醒", "语音命令"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-notifications",
        title_en: "Notification Permissions",
        title_zh: "通知权限",
        aliases: &["notification access", "通知访问"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-accountinfo",
        title_en: "Account Info",
        title_zh: "账户信息",
        aliases: &["account permissions", "账户权限"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-contacts",
        title_en: "Contacts",
        title_zh: "联系人",
        aliases: &["contacts access", "通讯录"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-calendar",
        title_en: "Calendar Access",
        title_zh: "日历访问权限",
        aliases: &["calendar permissions", "日历权限"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-phonecallhistory",
        title_en: "Call History",
        title_zh: "通话记录",
        aliases: &["call history access", "电话历史"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-email",
        title_en: "Email Access",
        title_zh: "电子邮件访问权限",
        aliases: &["email permissions", "邮件权限"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-tasks",
        title_en: "Tasks",
        title_zh: "任务",
        aliases: &["task access", "任务权限"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-messaging",
        title_en: "Messaging",
        title_zh: "消息",
        aliases: &["sms", "messaging access", "短信", "消息权限"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-radios",
        title_en: "Radios",
        title_zh: "无线电",
        aliases: &[
            "radio control",
            "wireless permissions",
            "无线电权限",
            "无线控制",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-customdevices",
        title_en: "Other Devices",
        title_zh: "其他设备",
        aliases: &["device permissions", "other device access", "设备权限"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-backgroundapps",
        title_en: "Background Apps",
        title_zh: "后台应用",
        aliases: &["background apps", "run in background", "后台运行"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-appdiagnostics",
        title_en: "App Diagnostics",
        title_zh: "应用诊断",
        aliases: &["app diagnostics", "diagnostic info", "诊断信息"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-automaticfiledownloads",
        title_en: "Automatic File Downloads",
        title_zh: "自动文件下载",
        aliases: &["automatic downloads", "file downloads", "自动下载"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-documents",
        title_en: "Documents",
        title_zh: "文档",
        aliases: &["document access", "文档访问权限"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-downloads",
        title_en: "Downloads Folder",
        title_zh: "下载文件夹",
        aliases: &["downloads access", "下载目录", "下载权限"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-broadfilesystemaccess",
        title_en: "File System",
        title_zh: "文件系统",
        aliases: &["file system access", "files", "文件访问权限", "文件"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-pictures",
        title_en: "Pictures",
        title_zh: "图片",
        aliases: &["picture access", "photos", "照片", "图片权限"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-videos",
        title_en: "Videos",
        title_zh: "视频",
        aliases: &["video access", "视频权限"],
    },
    SettingsEntry {
        uri: "ms-settings:privacy-screenshots",
        title_en: "Screenshots",
        title_zh: "屏幕截图",
        aliases: &[
            "screenshot access",
            "screen capture",
            "截图权限",
            "屏幕捕获",
        ],
    },
    SettingsEntry {
        uri: "ms-settings:delivery-optimization",
        title_en: "Delivery Optimization",
        title_zh: "传递优化",
        aliases: &[
            "download optimization",
            "update delivery",
            "下载优化",
            "更新传递",
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn uris(index: &SettingsIndex, query: &str) -> Vec<&'static str> {
        index
            .query(query)
            .into_iter()
            .map(|hit| hit.entry.uri)
            .collect()
    }

    #[test]
    fn empty_queries_do_not_flood_the_launcher_with_settings() {
        assert!(SettingsIndex::new().query("").is_empty());
        assert!(SettingsIndex::new().query("   ").is_empty());
    }

    #[test]
    fn english_titles_and_aliases_find_specific_pages() {
        let index = SettingsIndex::new();
        assert_eq!(uris(&index, "settings").first(), Some(&"ms-settings:"));
        assert_eq!(
            uris(&index, "bluetooth").first(),
            Some(&"ms-settings:bluetooth")
        );
        assert!(uris(&index, "wifi").contains(&"ms-settings:network-wifi"));
        assert!(uris(&index, "dark mode").contains(&"ms-settings:personalization-colors"));
    }

    #[test]
    fn chinese_titles_and_pinyin_find_settings() {
        let index = SettingsIndex::new();
        assert!(uris(&index, "蓝牙").contains(&"ms-settings:bluetooth"));
        assert!(uris(&index, "lanya").contains(&"ms-settings:bluetooth"));
        assert!(uris(&index, "xianshi").contains(&"ms-settings:display"));
    }

    #[test]
    fn result_rows_use_localized_titles_and_deep_link_actions() {
        let index = SettingsIndex::new();
        let mut source = SettingsSource {
            index: &index,
            label: "Windows Settings",
            locale: "zh-CN",
        };
        let icon = Some("data:image/png;base64,test".into());
        let contribution = source.contribute_with_icon("wifi", icon.clone());
        let row = contribution
            .rows
            .iter()
            .find(|row| row.id == "settings:ms-settings:network-wifi")
            .unwrap();
        assert_eq!(row.title, "Wi-Fi");
        assert_eq!(row.subtitle, "Windows Settings");
        assert_eq!(row.icon, icon);
        assert!(contribution.actions.iter().any(|action| matches!(
            action,
            RowAction::OpenSettings(uri) if uri == "ms-settings:network-wifi"
        )));
    }

    #[test]
    fn catalog_ids_are_unique() {
        let mut uris = SETTINGS.iter().map(|entry| entry.uri).collect::<Vec<_>>();
        let len = uris.len();
        uris.sort_unstable();
        uris.dedup();
        assert_eq!(uris.len(), len);
    }
}
