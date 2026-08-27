//! Trigger routing: turn a launcher query into the plugin commands that may
//! answer it, *before* any plugin process is woken.
//!
//! M2 supports the four manifest trigger kinds with a strict precedence:
//! exact command name, keyword prefix (longest prefix wins), regex, and
//! dynamic (every query, subject to the 100 ms response timeout). A plugin is
//! only invoked when a route actually matches, so cold-start and search cost
//! stay proportional to the active plugin count, never the installed one.

use steward_plugin_registry::{PluginCommand, TriggerType};

/// A deadline for a static (non-dynamic) command, in milliseconds. Dynamic
/// commands get the circuit-break deadline instead (see
/// [`RouteHit::DYNAMIC_DEADLINE_MS`]).
pub const STATIC_DEADLINE_MS: u64 = 500;
/// Circuit-break deadline for dynamic commands: if the plugin has not answered
/// within 100 ms the runtime kills the isolate and this render skips the row.
pub const DYNAMIC_DEADLINE_MS: u64 = 100;

/// One query match: the plugin command to invoke plus the input to pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteHit {
    pub plugin_id: String,
    pub command: String,
    /// Human-readable command title (the row label before the view returns).
    pub title: String,
    /// Query text passed to the command: the full query for `command` /
    /// `regex` / `dynamic`, the text after the prefix for `prefix`.
    pub input: String,
    /// Execution deadline the host must honor for this hit.
    pub deadline_ms: u64,
}

/// A command registered for routing.
#[derive(Debug, Clone)]
struct Route {
    plugin_id: String,
    command: String,
    title: String,
    kind: TriggerType,
    /// Prefix or regex value; `None` for `command` / `dynamic`.
    value: Option<String>,
}

/// Routing tables for all loaded plugins. Pure data: no process or I/O, so it
/// is fully unit-testable and cheap to rebuild on plugin changes.
#[derive(Debug, Default)]
pub struct RouteIndex {
    commands: Vec<Route>,
    /// Prefix routes sorted by value length, longest first.
    prefixes: Vec<Route>,
    regexes: Vec<Route>,
    dynamics: Vec<Route>,
}

impl RouteIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register every command of one plugin. Duplicate command names within a
    /// plugin were rejected at manifest validation; routes across plugins may
    /// overlap and all matching routes fire.
    pub fn add_plugin(&mut self, plugin_id: &str, commands: &[PluginCommand]) {
        for command in commands {
            let route = Route {
                plugin_id: plugin_id.to_string(),
                command: command.name.clone(),
                title: command.title.clone(),
                kind: command.trigger.kind,
                value: command.trigger.value.clone(),
            };
            match command.trigger.kind {
                TriggerType::Command => self.commands.push(route),
                TriggerType::Prefix => self.prefixes.push(route),
                TriggerType::Regex => self.regexes.push(route),
                TriggerType::Dynamic => self.dynamics.push(route),
            }
        }
        // Longest prefix first: when several routes match, the most specific
        // (longest) prefix wins.
        self.prefixes
            .sort_by_key(|route| std::cmp::Reverse(route.value.as_deref().unwrap_or("").len()));
    }

    /// Drop every route belonging to a plugin (e.g. it was uninstalled).
    pub fn remove_plugin(&mut self, plugin_id: &str) {
        self.commands.retain(|route| route.plugin_id != plugin_id);
        self.prefixes.retain(|route| route.plugin_id != plugin_id);
        self.regexes.retain(|route| route.plugin_id != plugin_id);
        self.dynamics.retain(|route| route.plugin_id != plugin_id);
    }

    /// Drop all routes (used when rebuilding from a fresh scan).
    pub fn clear(&mut self) {
        self.commands.clear();
        self.prefixes.clear();
        self.regexes.clear();
        self.dynamics.clear();
    }

    /// Number of registered routes (all kinds).
    pub fn len(&self) -> usize {
        self.commands.len() + self.prefixes.len() + self.regexes.len() + self.dynamics.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Match a query against the routing tables, in precedence order: exact
    /// command name, keyword prefix, regex, dynamic.
    pub fn match_query(&self, query: &str) -> Vec<RouteHit> {
        let query = query.trim();
        if query.is_empty() {
            // Only dynamic routes participate in the empty query.
            return self.dynamics.iter().map(hit_for).collect();
        }

        let mut hits = Vec::new();
        for route in &self.commands {
            if matches_command(&route.command, query) {
                hits.push(RouteHit {
                    plugin_id: route.plugin_id.clone(),
                    command: route.command.clone(),
                    title: route.title.clone(),
                    input: query.to_string(),
                    deadline_ms: STATIC_DEADLINE_MS,
                });
            }
        }

        // Longest-prefix priority: among all routes whose prefix matches,
        // only those with the longest prefix fire (a shorter, generic prefix
        // must not shadow or crowd out a more specific one).
        let mut prefix_hits = Vec::new();
        let mut longest = 0;
        for route in &self.prefixes {
            let value = route.value.as_deref().unwrap_or("");
            if let Some(rest) = query.strip_prefix(value) {
                if value.len() > longest {
                    prefix_hits.clear();
                    longest = value.len();
                }
                if value.len() == longest {
                    prefix_hits.push(RouteHit {
                        plugin_id: route.plugin_id.clone(),
                        command: route.command.clone(),
                        title: route.title.clone(),
                        input: rest.trim().to_string(),
                        deadline_ms: STATIC_DEADLINE_MS,
                    });
                }
            }
        }
        hits.extend(prefix_hits);

        for route in &self.regexes {
            let pattern = route.value.as_deref().unwrap_or("");
            if let Ok(regex) = regex::Regex::new(pattern) {
                if regex.is_match(query) {
                    hits.push(RouteHit {
                        plugin_id: route.plugin_id.clone(),
                        command: route.command.clone(),
                        title: route.title.clone(),
                        input: query.to_string(),
                        deadline_ms: STATIC_DEADLINE_MS,
                    });
                }
            }
        }

        hits.extend(self.dynamics.iter().map(|route| RouteHit {
            plugin_id: route.plugin_id.clone(),
            command: route.command.clone(),
            title: route.title.clone(),
            input: query.to_string(),
            deadline_ms: DYNAMIC_DEADLINE_MS,
        }));
        hits
    }

    /// All routes as hits, regardless of query (used by tests / debugging).
    pub fn all_routes(&self) -> Vec<RouteHit> {
        self.commands
            .iter()
            .chain(&self.prefixes)
            .chain(&self.regexes)
            .chain(&self.dynamics)
            .map(hit_for)
            .collect()
    }
}

fn hit_for(route: &Route) -> RouteHit {
    RouteHit {
        plugin_id: route.plugin_id.clone(),
        command: route.command.clone(),
        title: route.title.clone(),
        input: String::new(),
        deadline_ms: match route.kind {
            TriggerType::Dynamic => DYNAMIC_DEADLINE_MS,
            _ => STATIC_DEADLINE_MS,
        },
    }
}

/// `command` triggers match the exact command name or the name followed by a
/// space (so `calendar tomorrow` reaches the calendar command), but never a
/// mere prefix of another word (`calendarx` does not match `calendar`).
fn matches_command(command: &str, query: &str) -> bool {
    query == command
        || query
            .strip_prefix(command)
            .is_some_and(|rest| rest.starts_with(' '))
}

#[cfg(test)]
mod tests {
    use super::*;
    use steward_plugin_registry::{PluginCommand, Trigger};

    fn command(name: &str, kind: TriggerType, value: Option<&str>) -> PluginCommand {
        PluginCommand {
            name: name.into(),
            title: format!("Title {name}"),
            trigger: Trigger {
                kind,
                value: value.map(str::to_string),
            },
        }
    }

    #[test]
    fn command_trigger_matches_exact_and_prefixed_queries() {
        let mut index = RouteIndex::new();
        index.add_plugin(
            "com.example.calendar",
            &[command("calendar", TriggerType::Command, None)],
        );
        let hits = index.match_query("calendar");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].input, "calendar");

        let hits = index.match_query("calendar tomorrow");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].input, "calendar tomorrow");

        assert!(index.match_query("calendarx").is_empty());
        assert!(index.match_query("calend").is_empty());
    }

    #[test]
    fn prefix_trigger_strips_prefix_and_prefers_longest() {
        let mut index = RouteIndex::new();
        index.add_plugin(
            "com.example.short",
            &[command("short", TriggerType::Prefix, Some("cal "))],
        );
        index.add_plugin(
            "com.example.long",
            &[command("long", TriggerType::Prefix, Some("calc "))],
        );
        let hits = index.match_query("calc something");
        assert_eq!(hits.len(), 1, "only the longest prefix may fire");
        assert_eq!(hits[0].plugin_id, "com.example.long");
        assert_eq!(hits[0].input, "something");

        let hits = index.match_query("cal something");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].plugin_id, "com.example.short");
        assert_eq!(hits[0].input, "something");
    }

    #[test]
    fn regex_trigger_matches_pattern() {
        let mut index = RouteIndex::new();
        index.add_plugin(
            "com.example.regex",
            &[command("match", TriggerType::Regex, Some(r"^open github/"))],
        );
        assert_eq!(index.match_query("open github/steward").len(), 1);
        assert!(index.match_query("open gitlab/steward").is_empty());
    }

    #[test]
    fn dynamic_routes_always_participate_with_short_deadline() {
        let mut index = RouteIndex::new();
        index.add_plugin(
            "com.example.dynamic",
            &[command("search", TriggerType::Dynamic, None)],
        );
        let empty = index.match_query("");
        assert_eq!(empty.len(), 1);
        assert_eq!(empty[0].deadline_ms, DYNAMIC_DEADLINE_MS);
        let full = index.match_query("anything at all");
        assert_eq!(full.len(), 1);
        assert_eq!(full[0].input, "anything at all");
    }

    #[test]
    fn precedence_is_command_then_prefix_then_regex_then_dynamic() {
        let mut index = RouteIndex::new();
        index.add_plugin("dyn", &[command("d", TriggerType::Dynamic, None)]);
        index.add_plugin("regex", &[command("r", TriggerType::Regex, Some("cal"))]);
        index.add_plugin("prefix", &[command("p", TriggerType::Prefix, Some("cal "))]);
        index.add_plugin("cmd", &[command("cal", TriggerType::Command, None)]);
        let hits = index.match_query("cal x");
        let kinds = hits
            .iter()
            .map(|hit| hit.plugin_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(kinds, vec!["cmd", "prefix", "regex", "dyn"]);
    }

    #[test]
    fn remove_plugin_drops_all_routes() {
        let mut index = RouteIndex::new();
        index.add_plugin(
            "com.example.one",
            &[
                command("a", TriggerType::Command, None),
                command("b", TriggerType::Dynamic, None),
            ],
        );
        index.add_plugin(
            "com.example.two",
            &[command("c", TriggerType::Command, None)],
        );
        assert_eq!(index.len(), 3);
        index.remove_plugin("com.example.one");
        assert_eq!(index.len(), 1);
        assert!(index.match_query("a").is_empty());
        assert_eq!(index.match_query("c").len(), 1);
    }

    #[test]
    fn large_index_matches_without_degradation() {
        // Cold-start and search cost must scale with active plugins, not the
        // installed count: a 1000-command index matches in one pass.
        let mut index = RouteIndex::new();
        let mut commands = Vec::new();
        for i in 0..1000 {
            commands.push(command(&format!("cmd{i}"), TriggerType::Command, None));
        }
        index.add_plugin("com.example.many", &commands);
        assert_eq!(index.len(), 1000);
        assert_eq!(index.match_query("cmd999").len(), 1);
    }
}
