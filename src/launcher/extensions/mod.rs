//! Quicklinks (URL templates) and shell commands, found and run from the palette.
//!
//! Both are matched the same way. Typing `<keyword> <argument>` (`g yorkshire pudding recipe`) fills
//! `{query}` and puts the result above every fuzzy match. Otherwise the name and keyword are
//! fuzzy-matched like apps; one that takes `{query}` but has none yet completes the search
//! field to `<keyword> ` instead of opening. Without a keyword, the name doubles as one.

mod runner;
mod template;

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use objc2::rc::autoreleasepool;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSString, NSURL};

use super::fuzzy::{self, Prepared};
use super::{KEYWORD_SCORE, LauncherObserver, OpenOutcome, ResultKind, SearchResult};
use crate::config::{CommandOutput, LauncherConfig, Quicklink, ShellCommand};
use crate::error::CoreError;
use template::{CLIPBOARD, QUERY};

/// Typing a keyword exactly lifts it above fuzzy matches on app names that start the same
/// way (`g` → Google before GarageBand) without outranking a keyword with an argument.
const KEYWORD_EXACT_BONUS: i32 = 150;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const SUBTITLE_MAX: usize = 80;
const ARGUMENT_IN_TITLE_MAX: usize = 60;
const SHELL: &str = "/bin/zsh";

/// A quicklink or a command, seen the same way for matching.
struct Entry<'a> {
    id: String,
    name: &'a str,
    keyword: &'a str,
    template: &'a str,
    kind: ResultKind,
    needs_confirmation: bool,
}

impl<'a> Entry<'a> {
    fn quicklink(link: &'a Quicklink) -> Self {
        Self {
            id: format!("quicklink:{}", link.id),
            name: &link.name,
            keyword: &link.keyword,
            template: &link.url,
            kind: ResultKind::Quicklink,
            needs_confirmation: false,
        }
    }

    fn command(command: &'a ShellCommand) -> Self {
        Self {
            id: format!("command:{}", command.id),
            name: &command.name,
            keyword: &command.keyword,
            template: &command.command,
            kind: ResultKind::Command,
            needs_confirmation: command.needs_confirmation,
        }
    }

    /// The keyword, or the name when there isn't one, so every entry can take an argument.
    fn trigger(&self) -> &'a str {
        if self.keyword.trim().is_empty() {
            self.name.trim()
        } else {
            self.keyword.trim()
        }
    }

    fn takes_argument(&self) -> bool {
        template::uses(self.template, QUERY)
    }

    fn title_with(&self, argument: &str) -> String {
        let argument = template::truncate(argument, ARGUMENT_IN_TITLE_MAX);
        match self.kind {
            ResultKind::Command => format!("Run {} with “{argument}”", self.name),
            _ => format!("{} for “{argument}”", self.name),
        }
    }

    /// The template with `{query}` filled as it will be on opening. `{clipboard}` stays as
    /// typed: reading the pasteboard on every keystroke isn't worth it for a preview.
    fn preview(&self, argument: &str) -> String {
        let filled = match self.kind {
            ResultKind::Command => fill_command(self.template, argument, None),
            _ => fill_quicklink(self.template, argument, None),
        };
        template::truncate(&filled, SUBTITLE_MAX)
    }

    fn result(&self, title: String, subtitle: String, completion: Option<String>) -> SearchResult {
        SearchResult {
            id: self.id.clone(),
            title,
            subtitle,
            kind: self.kind,
            path: None,
            symbol: Some(
                match self.kind {
                    ResultKind::Command => "terminal",
                    _ => "link",
                }
                .into(),
            ),
            needs_confirmation: self.needs_confirmation,
            completion,
            hotkey: None,
            is_favourite: false,
        }
    }
}

fn entries(config: &LauncherConfig) -> impl Iterator<Item = Entry<'_>> {
    config
        .quicklinks
        .iter()
        .map(Entry::quicklink)
        .chain(config.commands.iter().map(Entry::command))
}

/// Scored results for `query` (raw, as typed). Ids are `quicklink:<id>` / `command:<id>`.
/// `<keyword> <argument>` scores `KEYWORD_SCORE`; name matches use fuzzy scores like apps.
pub(crate) fn results(query: &str, config: &LauncherConfig) -> Vec<(i32, SearchResult)> {
    let folded = fuzzy::fold_query(query);
    entries(config)
        .filter_map(|entry| score(&entry, query, &folded))
        .collect()
}

/// Every quicklink and command as listed with an empty query, in configured order.
pub(crate) fn all(config: &LauncherConfig) -> Vec<SearchResult> {
    entries(config).map(|entry| listing(&entry)).collect()
}

fn score(entry: &Entry, query: &str, folded: &[char]) -> Option<(i32, SearchResult)> {
    if entry.takes_argument()
        && let Some(argument) = template::split_argument(query, entry.trigger())
    {
        // Only the keyword and a space so far: opening runs it with an empty argument.
        let title = if argument.is_empty() {
            entry.name.to_owned()
        } else {
            entry.title_with(argument)
        };
        let subtitle = entry.preview(argument);
        return Some((KEYWORD_SCORE, entry.result(title, subtitle, None)));
    }

    let name = Prepared::new(entry.name);
    let keyword = (!entry.keyword.trim().is_empty()).then(|| Prepared::new(entry.keyword));
    let mut score = super::best_score(folded, &name, keyword.iter())?;
    if keyword.is_some() && folded == fuzzy::fold_query(entry.keyword).as_slice() {
        score += KEYWORD_EXACT_BONUS;
    }
    Some((score, listing(entry)))
}

/// The entry by name with its template as subtitle; one taking an argument completes to
/// its keyword.
fn listing(entry: &Entry) -> SearchResult {
    let completion = entry
        .takes_argument()
        .then(|| format!("{} ", entry.trigger()));
    let subtitle = template::truncate(entry.template.trim(), SUBTITLE_MAX);
    entry.result(entry.name.to_owned(), subtitle, completion)
}

/// Opens a quicklink or runs a command. `query` is what was typed, from which the argument
/// is taken. Must return quickly: commands run on a background thread and report through
/// `observer.command_finished`.
pub(crate) fn open(
    id: &str,
    query: &str,
    config: &LauncherConfig,
    observer: Option<Arc<dyn LauncherObserver>>,
) -> Result<OpenOutcome, CoreError> {
    if let Some(link_id) = id.strip_prefix("quicklink:") {
        let link = config
            .quicklinks
            .iter()
            .find(|l| l.id == link_id)
            .ok_or_else(|| missing("quicklink"))?;
        let entry = Entry::quicklink(link);
        let clipboard = read_clipboard_if_used(&link.url);
        let url = fill_quicklink(&link.url, argument(&entry, query), clipboard.as_deref());
        open_target(resolve_target(&url).map_err(|message| CoreError::Other { message })?)?;
        Ok(OpenOutcome::Done)
    } else if let Some(command_id) = id.strip_prefix("command:") {
        let command = config
            .commands
            .iter()
            .find(|c| c.id == command_id)
            .ok_or_else(|| missing("command"))?;
        let entry = Entry::command(command);
        let clipboard = read_clipboard_if_used(&command.command);
        let script = fill_command(
            &command.command,
            argument(&entry, query),
            clipboard.as_deref(),
        );
        spawn_command(command.name.clone(), script, command.output, observer)?;
        Ok(OpenOutcome::Done)
    } else {
        Err(CoreError::Other {
            message: format!("unknown result {id}"),
        })
    }
}

fn missing(what: &str) -> CoreError {
    CoreError::Other {
        message: format!("That {what} no longer exists"),
    }
}

/// The typed argument, or empty when the entry was chosen by name.
fn argument<'q>(entry: &Entry, query: &'q str) -> &'q str {
    if entry.takes_argument() {
        template::split_argument(query, entry.trigger()).unwrap_or("")
    } else {
        ""
    }
}

fn read_clipboard_if_used(template: &str) -> Option<String> {
    if template::uses(template, CLIPBOARD) {
        Some(crate::pasteboard::paste().unwrap_or_default())
    } else {
        None
    }
}

// MARK: Quicklinks

/// File-path templates take values as typed; URLs take them percent-encoded.
fn fill_quicklink(url: &str, argument: &str, clipboard: Option<&str>) -> String {
    let encode = |text: &str| {
        if is_path(url) {
            text.to_owned()
        } else {
            template::url_encode(text)
        }
    };
    template::expand(url, |name| match name {
        QUERY => Some(encode(argument)),
        CLIPBOARD => clipboard.map(encode),
        _ => None,
    })
}

fn is_path(text: &str) -> bool {
    let text = text.trim_start();
    text.starts_with('/') || text.starts_with('~')
}

#[derive(Debug, PartialEq)]
enum Target {
    Url(String),
    File(PathBuf),
}

/// What a filled-in quicklink opens. Any scheme is allowed (`raycast://`, `obsidian://`,
/// `mailto:`), absolute and `~` paths open as files, and a bare `example.com` gets `https://`.
fn resolve_target(text: &str) -> Result<Target, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("The quicklink's URL is empty".into());
    }
    if text.starts_with('/') {
        return Ok(Target::File(PathBuf::from(text)));
    }
    if let Some(rest) = text.strip_prefix('~') {
        let home = std::env::var("HOME").unwrap_or_default();
        return Ok(Target::File(PathBuf::from(format!("{home}{rest}"))));
    }
    if has_scheme(text) {
        return Ok(Target::Url(text.to_owned()));
    }
    if text.contains('.') && !text.contains(char::is_whitespace) {
        return Ok(Target::Url(format!("https://{text}")));
    }
    Err(format!(
        "“{text}” isn't a URL; start it with a scheme like https://"
    ))
}

/// RFC 3986: a letter, then letters, digits, `+`, `-` or `.`, then `:`.
fn has_scheme(text: &str) -> bool {
    let Some((scheme, _)) = text.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

fn open_target(target: Target) -> Result<(), CoreError> {
    autoreleasepool(|_| {
        let (url, shown) = match &target {
            Target::Url(text) => (
                NSURL::URLWithString(&NSString::from_str(text)),
                text.clone(),
            ),
            Target::File(path) => {
                let path = path.to_string_lossy();
                (
                    Some(NSURL::fileURLWithPath(&NSString::from_str(&path))),
                    path.into_owned(),
                )
            }
        };
        let url = url.ok_or_else(|| CoreError::Other {
            message: format!("“{shown}” isn't a valid URL"),
        })?;
        if NSWorkspace::sharedWorkspace().openURL(&url) {
            Ok(())
        } else {
            Err(CoreError::Other {
                message: format!("Couldn't open “{shown}”"),
            })
        }
    })
}

// MARK: Commands

/// Substituted values are single-quoted, so templates use bare placeholders: `say {query}`,
/// not `say "{query}"` (which would say the quotes too).
fn fill_command(command: &str, argument: &str, clipboard: Option<&str>) -> String {
    template::expand(command, |name| match name {
        QUERY => Some(template::shell_quote(argument)),
        CLIPBOARD => clipboard.map(template::shell_quote),
        _ => None,
    })
}

/// Runs `script` with a login zsh (so the user's PATH applies) from the home folder, on a
/// background thread, then reports according to `output`.
fn spawn_command(
    name: String,
    script: String,
    output: CommandOutput,
    observer: Option<Arc<dyn LauncherObserver>>,
) -> Result<(), CoreError> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    thread::Builder::new()
        .name("turbo-command".into())
        .spawn(move || {
            let result = runner::run(SHELL, &["-lc", &script], &home, COMMAND_TIMEOUT);
            let (message, success) = match runner::report(output, &result) {
                runner::Report::Nothing => return,
                runner::Report::Notify { output, success } => (output, success),
                runner::Report::Copy(text) => match crate::pasteboard::copy(&text) {
                    Ok(()) => return,
                    Err(error) => (error.to_string(), false),
                },
            };
            if let Some(observer) = observer {
                observer.command_finished(name, message, success);
            }
        })
        .map(|_| ())
        .map_err(|error| CoreError::Other {
            message: format!("Couldn't start the command: {error}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(id: &str, name: &str, keyword: &str, url: &str) -> Quicklink {
        Quicklink {
            id: id.into(),
            name: name.into(),
            keyword: keyword.into(),
            url: url.into(),
        }
    }

    fn command(id: &str, name: &str, keyword: &str, script: &str) -> ShellCommand {
        ShellCommand {
            id: id.into(),
            name: name.into(),
            keyword: keyword.into(),
            command: script.into(),
            output: CommandOutput::Show,
            needs_confirmation: true,
        }
    }

    fn config() -> LauncherConfig {
        LauncherConfig {
            quicklinks: vec![
                link(
                    "google",
                    "Search Google",
                    "g",
                    "https://www.google.com/search?q={query}",
                ),
                link("news", "Hacker News", "hn", "https://news.ycombinator.com"),
                link("notes", "Daily Note", "", "obsidian://daily?vault={query}"),
            ],
            commands: vec![command("say", "Say", "say", "say {query}")],
            ..LauncherConfig::default()
        }
    }

    fn find<'a>(results: &'a [(i32, SearchResult)], id: &str) -> Option<&'a (i32, SearchResult)> {
        results.iter().find(|(_, r)| r.id == id)
    }

    #[test]
    fn keyword_with_argument_ranks_above_everything_but_the_calculator() {
        let results = results("g yorkshire pudding recipe", &config());
        let (score, result) = find(&results, "quicklink:google").unwrap();
        assert_eq!(*score, KEYWORD_SCORE);
        assert_eq!(result.title, "Search Google for “yorkshire pudding recipe”");
        assert_eq!(
            result.subtitle,
            "https://www.google.com/search?q=yorkshire%20pudding%20recipe"
        );
        assert_eq!(result.kind, ResultKind::Quicklink);
        assert_eq!(result.symbol.as_deref(), Some("link"));
        assert_eq!(result.completion, None);
    }

    #[test]
    fn command_argument_is_quoted_in_the_preview() {
        let results = results("say it's late", &config());
        let (score, result) = find(&results, "command:say").unwrap();
        assert_eq!(*score, KEYWORD_SCORE);
        assert_eq!(result.title, "Run Say with “it's late”");
        assert_eq!(result.subtitle, r"say 'it'\''s late'");
        assert!(result.needs_confirmation);
        assert_eq!(result.symbol.as_deref(), Some("terminal"));
    }

    #[test]
    fn name_match_without_argument_completes_the_keyword() {
        let results = results("google", &config());
        let (score, result) = find(&results, "quicklink:google").unwrap();
        assert!(*score < KEYWORD_SCORE);
        assert_eq!(result.completion.as_deref(), Some("g "));
    }

    #[test]
    fn links_without_placeholder_open_directly() {
        let results = results("hacker", &config());
        let (_, result) = find(&results, "quicklink:news").unwrap();
        assert_eq!(result.completion, None);
        // A keyword plus text isn't an argument when there's nowhere to put it.
        assert!(find(&super::results("hn rust", &config()), "quicklink:news").is_none());
    }

    #[test]
    fn exact_keyword_gets_a_bonus() {
        let exact = find(&results("g", &config()), "quicklink:google")
            .unwrap()
            .0;
        let plain = super::super::best_score(
            &fuzzy::fold_query("g"),
            &Prepared::new("Search Google"),
            [Prepared::new("g")].iter(),
        )
        .unwrap();
        assert_eq!(exact, plain + KEYWORD_EXACT_BONUS);
        assert!(exact < KEYWORD_SCORE);
    }

    #[test]
    fn name_doubles_as_keyword_when_there_is_none() {
        let results = results("daily note work", &config());
        let (score, result) = find(&results, "quicklink:notes").unwrap();
        assert_eq!(*score, KEYWORD_SCORE);
        assert_eq!(result.subtitle, "obsidian://daily?vault=work");
        let completion = find(&super::results("daily", &config()), "quicklink:notes")
            .unwrap()
            .1
            .completion
            .clone();
        assert_eq!(completion.as_deref(), Some("Daily Note "));
    }

    #[test]
    fn keyword_and_space_alone_opens_with_an_empty_argument() {
        let results = results("g ", &config());
        let (score, result) = find(&results, "quicklink:google").unwrap();
        assert_eq!(*score, KEYWORD_SCORE);
        assert_eq!(result.title, "Search Google");
        assert_eq!(result.completion, None);
    }

    #[test]
    fn unrelated_queries_match_nothing() {
        assert!(results("zzz", &config()).is_empty());
    }

    #[test]
    fn argument_comes_from_the_query() {
        let config = config();
        let google = Entry::quicklink(&config.quicklinks[0]);
        let news = Entry::quicklink(&config.quicklinks[1]);
        assert_eq!(argument(&google, "g rust"), "rust");
        assert_eq!(argument(&google, "google"), "");
        assert_eq!(argument(&news, "hn rust"), "");
    }

    #[test]
    fn quicklinks_encode_but_paths_do_not() {
        assert_eq!(
            fill_quicklink(
                "https://x.test/?q={query}&c={clipboard}",
                "a b",
                Some("c&d")
            ),
            "https://x.test/?q=a%20b&c=c%26d"
        );
        assert_eq!(
            fill_quicklink("~/Notes/{query}.md", "my note", None),
            "~/Notes/my note.md"
        );
        // An unread clipboard leaves its placeholder for the preview.
        assert_eq!(
            fill_quicklink("https://x.test/{clipboard}", "", None),
            "https://x.test/{clipboard}"
        );
    }

    #[test]
    fn commands_quote_substitutions() {
        assert_eq!(
            fill_command("echo {query} {clipboard}", "$(rm -rf ~)", Some("`id`")),
            "echo '$(rm -rf ~)' '`id`'"
        );
    }

    #[test]
    fn resolves_targets() {
        assert_eq!(
            resolve_target(" https://example.com/x "),
            Ok(Target::Url("https://example.com/x".into()))
        );
        assert_eq!(
            resolve_target("raycast://extensions"),
            Ok(Target::Url("raycast://extensions".into()))
        );
        assert_eq!(
            resolve_target("mailto:a@b.test"),
            Ok(Target::Url("mailto:a@b.test".into()))
        );
        assert_eq!(
            resolve_target("example.com/page"),
            Ok(Target::Url("https://example.com/page".into()))
        );
        assert_eq!(
            resolve_target("/Applications"),
            Ok(Target::File("/Applications".into()))
        );
        let home = std::env::var("HOME").unwrap_or_default();
        assert_eq!(
            resolve_target("~/Desktop"),
            Ok(Target::File(format!("{home}/Desktop").into()))
        );
        assert!(resolve_target("").is_err());
        assert!(resolve_target("just words").is_err());
        assert!(resolve_target("1http://x").is_err());
    }

    #[test]
    fn unknown_ids_are_errors() {
        let config = config();
        assert!(open("quicklink:nope", "", &config, None).is_err());
        assert!(open("command:nope", "", &config, None).is_err());
        assert!(open("app:x", "", &config, None).is_err());
    }
}
