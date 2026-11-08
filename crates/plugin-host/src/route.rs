//! Route launcher queries without loading any plugin VM.

use nucleo::{Config, Matcher, Utf32Str, Utf32String};
use steward_plugin_registry::{PluginCommand, TriggerType};
use steward_search::search_haystacks;

const MAX_ROUTE_HAYSTACKS: usize = 32;
const MAX_ROUTE_HITS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteHit {
    pub plugin_id: String,
    pub command: String,
    pub title: String,
    pub input: String,
    pub fuzzy: bool,
    pub score: u16,
}

#[derive(Debug, Clone)]
struct Route {
    plugin_id: String,
    command: String,
    title: String,
    value: Option<String>,
    kind: TriggerType,
    regex: Option<regex::Regex>,
    haystacks: Vec<Utf32String>,
    haystack_texts: Vec<String>,
}

#[derive(Debug, Default)]
pub struct RouteIndex {
    commands: Vec<Route>,
    prefixes: Vec<Route>,
    regexes: Vec<Route>,
}

impl RouteIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.commands.clear();
        self.prefixes.clear();
        self.regexes.clear();
    }

    pub fn add_plugin(&mut self, plugin_id: &str, commands: &[PluginCommand]) {
        for command in commands {
            let haystacks = haystacks(command);
            let haystack_texts = haystacks
                .iter()
                .map(|text| text.to_string().to_lowercase())
                .collect();
            let route = Route {
                plugin_id: plugin_id.to_owned(),
                command: command.name.clone(),
                title: command.title.clone(),
                value: command.trigger.value.clone(),
                kind: command.trigger.kind,
                regex: (command.trigger.kind == TriggerType::Regex)
                    .then_some(command.trigger.value.as_deref())
                    .flatten()
                    .and_then(|pattern| regex::Regex::new(pattern).ok()),
                haystacks,
                haystack_texts,
            };
            match route.kind {
                TriggerType::Command => self.commands.push(route),
                TriggerType::Prefix => self.prefixes.push(route),
                TriggerType::Regex => self.regexes.push(route),
            }
        }
        self.prefixes
            .sort_by_key(|route| std::cmp::Reverse(route.value.as_deref().unwrap_or("").len()));
    }

    pub fn len(&self) -> usize {
        self.commands.len() + self.prefixes.len() + self.regexes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn match_query(&self, query: &str) -> Vec<RouteHit> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        let mut hits = Vec::new();
        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut buffer = Vec::new();
        let needle_text = query.to_lowercase();
        let needle = Utf32Str::new(&needle_text, &mut buffer);
        let mut command_hits = Vec::new();
        for route in &self.commands {
            if let Some(score) = command_score(route, query, needle, &mut matcher) {
                command_hits.push((
                    score,
                    RouteHit {
                        plugin_id: route.plugin_id.clone(),
                        command: route.command.clone(),
                        title: route.title.clone(),
                        input: query.to_owned(),
                        fuzzy: !strong_match(route, &needle_text),
                        score,
                    },
                ));
            }
        }
        command_hits.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        hits.extend(command_hits.into_iter().map(|(_, hit)| hit));

        let mut longest = 0;
        let mut prefix_hits = Vec::new();
        for route in &self.prefixes {
            let prefix = route.value.as_deref().unwrap_or("");
            if let Some(rest) = query.strip_prefix(prefix) {
                if prefix.len() > longest {
                    longest = prefix.len();
                    prefix_hits.clear();
                }
                if prefix.len() == longest {
                    prefix_hits.push(RouteHit {
                        plugin_id: route.plugin_id.clone(),
                        command: route.command.clone(),
                        title: route.title.clone(),
                        input: rest.trim().to_owned(),
                        fuzzy: false,
                        score: u16::MAX,
                    });
                }
            }
        }
        hits.extend(prefix_hits);

        for route in &self.regexes {
            if let Some(regex) = &route.regex {
                if regex.is_match(query) {
                    hits.push(RouteHit {
                        plugin_id: route.plugin_id.clone(),
                        command: route.command.clone(),
                        title: route.title.clone(),
                        input: query.to_owned(),
                        fuzzy: false,
                        score: u16::MAX,
                    });
                }
            }
        }
        hits.truncate(MAX_ROUTE_HITS);
        hits
    }
}

fn haystacks(command: &PluginCommand) -> Vec<Utf32String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for text in std::iter::once(&command.name)
        .chain(std::iter::once(&command.title))
        .chain(&command.keywords)
    {
        for variant in search_haystacks(text) {
            if seen.insert(variant.to_string().to_lowercase()) {
                out.push(variant);
                if out.len() == MAX_ROUTE_HAYSTACKS {
                    return out;
                }
            }
        }
    }
    out
}

fn command_score(
    route: &Route,
    query: &str,
    needle: Utf32Str<'_>,
    matcher: &mut Matcher,
) -> Option<u16> {
    if query == route.command
        || query
            .strip_prefix(&route.command)
            .is_some_and(|rest| rest.starts_with(' '))
    {
        return Some(u16::MAX);
    }
    route
        .haystacks
        .iter()
        .filter_map(|text| matcher.fuzzy_match(text.slice(..), needle))
        .max()
}

fn strong_match(route: &Route, needle: &str) -> bool {
    route
        .haystack_texts
        .iter()
        .any(|text| text.starts_with(needle) || needle.starts_with(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use steward_plugin_registry::{PluginCommand, Trigger};

    fn command(name: &str, kind: TriggerType, value: Option<&str>) -> PluginCommand {
        PluginCommand {
            name: name.into(),
            title: name.into(),
            keywords: Vec::new(),
            trigger: Trigger {
                kind,
                value: value.map(str::to_owned),
            },
        }
    }

    #[test]
    fn command_prefix_regex_and_precedence() {
        let mut index = RouteIndex::new();
        index.add_plugin(
            "p",
            &[
                command("calendar", TriggerType::Command, None),
                command("calc", TriggerType::Prefix, Some("= ")),
                command("url", TriggerType::Regex, Some("^https://")),
            ],
        );
        assert_eq!(index.match_query("calendar")[0].plugin_id, "p");
        assert_eq!(index.match_query("= 2+2")[0].input, "2+2");
        assert_eq!(index.match_query("https://example.com")[0].command, "url");
        assert!(index.match_query("unknown").is_empty());
    }

    #[test]
    fn longest_prefix_wins() {
        let mut index = RouteIndex::new();
        index.add_plugin("short", &[command("s", TriggerType::Prefix, Some("ca "))]);
        index.add_plugin("long", &[command("l", TriggerType::Prefix, Some("cal "))]);
        assert_eq!(index.match_query("cal x")[0].plugin_id, "long");
    }

    #[test]
    fn results_are_capped_per_query() {
        let mut index = RouteIndex::new();
        let commands = (0..40)
            .map(|index| command(&format!("run{index}"), TriggerType::Command, None))
            .collect::<Vec<_>>();
        index.add_plugin("p", &commands);
        assert_eq!(index.match_query("run").len(), MAX_ROUTE_HITS);
    }

    #[test]
    fn invalid_regexes_are_never_returned() {
        let mut index = RouteIndex::new();
        index.add_plugin("p", &[command("broken", TriggerType::Regex, Some("["))]);
        assert!(index.match_query("anything").is_empty());
    }
}
