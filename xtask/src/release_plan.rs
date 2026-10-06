//! Release planning (REL-011, REL-018):
//! - `release-plan`: the conventional commits since the last stable `v*` tag, the bump they imply
//!   and the next version, as JSON
//! - `release-verify`: the checks of the `verify` job of `release.yml`, run before anything is pushed

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, bail};
use semver::Version;
use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::{Date, OffsetDateTime, UtcOffset};

use crate::release::validate_version;
use crate::util::{Args, cargo, output, workspace_root};

const REQUIREMENT_PREFIXES: &[&str] = &[
    "ENG", "CON", "CDT", "LOG", "TRM", "STA", "IMG", "VOL", "NET", "SET", "SHL", "KBD", "NFR",
    "REL", "UPD",
];
/// Commit types that never reach users (REL-011).
const INTERNAL_TYPES: &[&str] = &["docs", "test", "chore", "ci", "style", "build"];
/// Scopes that never reach users, whatever the type (REL-011).
const INTERNAL_SCOPES: &[&str] = &["ci", "release", "docs", "spec", "deps"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    None,
    Patch,
    Minor,
    Major,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Patch => "patch",
            Self::Minor => "minor",
            Self::Major => "major",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Subject {
    kind: String,
    scope: Option<String>,
    breaking: bool,
    description: String,
    pr: Option<u32>,
}

struct Commit {
    sha: String,
    short: String,
    date: String,
    subject: String,
    parsed: Subject,
    breaking: bool,
    ids: BTreeSet<String>,
    body_ids: BTreeSet<String>,
    files: Vec<String>,
    visible: bool,
    level: Level,
    body: String,
}

struct Decision {
    bump: Level,
    next: Option<Version>,
    reason: String,
    internal_only: bool,
    warnings: Vec<String>,
}

// ── subjects ────────────────────────────────────────────────────────────────────────────────

fn parse_subject(subject: &str) -> Subject {
    let (text, pr) = split_pr(subject);
    match parse_header(text) {
        Some((kind, scope, breaking, description)) => Subject {
            kind,
            scope,
            breaking,
            description,
            pr,
        },
        None => Subject {
            kind: "other".to_owned(),
            scope: None,
            breaking: false,
            description: text.to_owned(),
            pr,
        },
    }
}

/// `subject (#123)` → (`subject`, 123).
fn split_pr(subject: &str) -> (&str, Option<u32>) {
    let text = subject.trim_end();
    if let Some(inner) = text.strip_suffix(')')
        && let Some(at) = inner.rfind(" (#")
        && let digits = &inner[at + 3..]
        && !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && let Ok(pr) = digits.parse()
    {
        return (text[..at].trim_end(), Some(pr));
    }
    (text, None)
}

type Header = (String, Option<String>, bool, String);

/// `type(scope)!: description`.
fn parse_header(text: &str) -> Option<Header> {
    let (head, description) = text.split_once(": ")?;
    let (head, breaking) = head.strip_suffix('!').map_or((head, false), |h| (h, true));
    let (kind, scope) = match head.split_once('(') {
        Some((kind, rest)) => (kind, Some(rest.strip_suffix(')')?)),
        None => (head, None),
    };
    let kind_ok = !kind.is_empty() && kind.bytes().all(|b| b.is_ascii_lowercase());
    let scope_ok = scope.is_none_or(|s| !s.is_empty() && !s.contains(['(', ')']));
    let description = description.trim();
    if !kind_ok || !scope_ok || description.is_empty() {
        return None;
    }
    Some((
        kind.to_owned(),
        scope.map(str::to_owned),
        breaking,
        description.to_owned(),
    ))
}

fn body_breaking(body: &str) -> bool {
    body.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("BREAKING CHANGE:") || line.starts_with("BREAKING-CHANGE:")
    })
}

/// Requirement ids (`REL-011`) of the known prefixes, as whole words.
fn requirement_ids(text: &str) -> BTreeSet<String> {
    let bytes = text.as_bytes();
    let mut ids = BTreeSet::new();
    let mut i = 0;
    while i < bytes.len() {
        let word_start =
            bytes[i].is_ascii_uppercase() && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric());
        if !word_start {
            i += 1;
            continue;
        }
        let mut end = i;
        while end < bytes.len() && bytes[end].is_ascii_uppercase() {
            end += 1;
        }
        if end + 4 <= bytes.len()
            && bytes[end] == b'-'
            && bytes[end + 1..end + 4].iter().all(u8::is_ascii_digit)
            && bytes
                .get(end + 4)
                .is_none_or(|b| !b.is_ascii_alphanumeric())
            && REQUIREMENT_PREFIXES.contains(&&text[i..end])
        {
            ids.insert(text[i..end + 4].to_owned());
        }
        i = end.max(i + 1);
    }
    ids
}

fn internal_scope(scope: Option<&str>) -> bool {
    scope.is_some_and(|s| s.split(',').all(|t| INTERNAL_SCOPES.contains(&t.trim())))
}

fn user_visible(subject: &Subject, breaking: bool) -> bool {
    breaking
        || (!INTERNAL_TYPES.contains(&subject.kind.as_str())
            && !internal_scope(subject.scope.as_deref()))
}

fn level_of(subject: &Subject, breaking: bool, zero_major: bool) -> Level {
    if !user_visible(subject, breaking) {
        Level::None
    } else if breaking {
        if zero_major {
            Level::Minor
        } else {
            Level::Major
        }
    } else if subject.kind == "feat" {
        Level::Minor
    } else {
        Level::Patch
    }
}

// ── versions ────────────────────────────────────────────────────────────────────────────────

fn core(v: &Version) -> Version {
    Version::new(v.major, v.minor, v.patch)
}

fn bumped(v: &Version, level: Level) -> Version {
    match level {
        Level::Major => Version::new(v.major + 1, 0, 0),
        Level::Minor => Version::new(v.major, v.minor + 1, 0),
        Level::Patch | Level::None => Version::new(v.major, v.minor, v.patch + 1),
    }
}

/// The version `bump` implies on top of `base`; a pending pre-release of a higher version wins.
fn next_version(
    base: Option<&Version>,
    last: Option<&Version>,
    workspace: Option<&Version>,
    bump: Level,
) -> Option<Version> {
    if bump == Level::None {
        return None;
    }
    let Some(base) = base else {
        return workspace.map(core);
    };
    let next = bumped(base, bump);
    match last {
        Some(last) if !last.pre.is_empty() && core(last) >= next => Some(core(last)),
        _ => Some(next),
    }
}

/// `patch|minor|major` on top of `base`, or an explicit version that must exceed `last`.
fn requested_version(
    request: &str,
    base: Option<&Version>,
    last: Option<&Version>,
) -> anyhow::Result<Version> {
    let level = match request {
        "patch" => Some(Level::Patch),
        "minor" => Some(Level::Minor),
        "major" => Some(Level::Major),
        _ => None,
    };
    let version = match level {
        Some(level) => {
            let base = base.context("there is no previous release: pass an explicit version")?;
            if level == Level::Major && base.major == 0 {
                bail!(
                    "`major` would leave 0.x: pass the explicit version {} to do that",
                    Version::new(1, 0, 0)
                );
            }
            bumped(base, level)
        }
        None => {
            let text = request.strip_prefix('v').unwrap_or(request);
            validate_version(text)?;
            Version::parse(text).with_context(|| format!("invalid version `{request}`"))?
        }
    };
    if let Some(last) = last
        && version <= *last
    {
        bail!("the version {version} is not greater than the latest tag v{last}");
    }
    Ok(version)
}

fn workspace_version_of(manifest: &str) -> Option<String> {
    let mut in_section = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == "[workspace.package]";
        } else if in_section
            && let Some(rest) = line.strip_prefix("version")
            && let Some(value) = rest.trim_start().strip_prefix('=')
        {
            return Some(value.trim().trim_matches('"').to_owned());
        }
    }
    None
}

fn decide(
    base_tag: Option<&str>,
    base: Option<&Version>,
    last: Option<&Version>,
    workspace: Option<&Version>,
    commits: &[Commit],
    request: Option<&str>,
) -> anyhow::Result<Decision> {
    if request.is_some() && commits.is_empty() {
        bail!(
            "nothing to release: no commits since {}",
            base_tag.unwrap_or("the first commit")
        );
    }
    let bump = commits.iter().map(|c| c.level).max().unwrap_or(Level::None);
    let computed = next_version(base, last, workspace, bump);
    let mut warnings = Vec::new();
    let next = match request {
        Some(request) => {
            let version = requested_version(request, base, last)?;
            if bump == Level::None {
                warnings.push(format!(
                    "{} commit(s) since {}, none user-visible: releasing {version} anyway",
                    commits.len(),
                    base_tag.unwrap_or("the first commit")
                ));
            } else if let Some(computed) = &computed
                && version < *computed
            {
                warnings.push(format!(
                    "requested {version} is lower than the computed {bump} bump ({computed})",
                    bump = bump.as_str()
                ));
            }
            Some(version)
        }
        None => {
            if let (Some(next), Some(last)) = (&computed, last)
                && next <= last
            {
                warnings.push(format!(
                    "the computed version {next} is not greater than the latest tag v{last}"
                ));
            }
            computed
        }
    };
    let others: Vec<&str> = commits
        .iter()
        .filter(|c| c.parsed.kind == "other")
        .map(|c| c.short.as_str())
        .collect();
    if !others.is_empty() {
        warnings.push(format!(
            "not conventional commits (counted as user-visible patch changes): {}",
            others.join(", ")
        ));
    }
    Ok(Decision {
        bump,
        next,
        reason: reason(bump, base_tag, commits),
        internal_only: bump == Level::None && !commits.is_empty(),
        warnings,
    })
}

fn reason(bump: Level, base_tag: Option<&str>, commits: &[Commit]) -> String {
    let since = base_tag.unwrap_or("the first commit");
    if commits.is_empty() {
        return format!("no commits since {since}");
    }
    if bump == Level::None {
        return format!(
            "{} commit(s) since {since}, none user-visible (docs, tests, chores, ci)",
            commits.len()
        );
    }
    let drivers: Vec<&Commit> = commits.iter().filter(|c| c.level == bump).collect();
    let shown: Vec<String> = drivers
        .iter()
        .take(3)
        .map(|c| format!("{} {}", c.short, c.subject))
        .collect();
    let mut text = format!("{}: {}", bump.as_str(), shown.join("; "));
    if drivers.len() > 3 {
        let _ = write!(text, "; and {} more", drivers.len() - 3);
    }
    text
}

// ── git ─────────────────────────────────────────────────────────────────────────────────────

fn git_output(repo: &Path, args: &[&str]) -> anyhow::Result<String> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(repo);
    output(&mut cmd)
}

/// A revision is passed to git as one argument; keep it free of options and ranges.
fn validate_rev(name: &str, rev: &str) -> anyhow::Result<()> {
    let ok = !rev.is_empty()
        && rev.len() <= 128
        && !rev.starts_with('-')
        && !rev.contains("..")
        && rev
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '~' | '^'));
    if !ok {
        bail!("invalid {name} `{rev}`");
    }
    Ok(())
}

/// Every `v<semver>` tag, lowest first.
fn version_tags(repo: &Path) -> anyhow::Result<Vec<(String, Version)>> {
    let out = git_output(repo, &["tag", "--list", "v*"])?;
    let mut tags: Vec<(String, Version)> = out
        .lines()
        .filter_map(|tag| {
            let tag = tag.trim();
            let version = Version::parse(tag.strip_prefix('v')?).ok()?;
            Some((tag.to_owned(), version))
        })
        .collect();
    tags.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(tags)
}

struct RawCommit {
    sha: String,
    date: String,
    subject: String,
    body: String,
}

fn read_commits(repo: &Path, range: &str) -> anyhow::Result<Vec<RawCommit>> {
    let format = "--format=%H%x1f%cI%x1f%s%x1f%b%x1e";
    let out = git_output(repo, &["log", "--first-parent", "--reverse", format, range])?;
    out.split('\u{1e}')
        .map(|record| record.trim_start_matches(['\n', '\r']))
        .filter(|record| !record.trim().is_empty())
        .map(|record| {
            let mut fields = record.splitn(4, '\u{1f}');
            let mut next = || fields.next().context("unexpected `git log` output");
            Ok(RawCommit {
                sha: next()?.trim().to_owned(),
                date: utc_date(next()?.trim())?,
                subject: next()?.trim().to_owned(),
                body: next()?.trim().to_owned(),
            })
        })
        .collect()
}

fn utc_date(iso: &str) -> anyhow::Result<String> {
    let at =
        OffsetDateTime::parse(iso, &Rfc3339).with_context(|| format!("bad commit date `{iso}`"))?;
    Ok(at.to_offset(UtcOffset::UTC).date().to_string())
}

/// Paths changed by a (squash-merge) commit, against its first parent.
fn changed_files(repo: &Path, sha: &str) -> anyhow::Result<Vec<String>> {
    let parent = format!("{sha}^1");
    let out = match git_output(repo, &["diff", "--name-only", "-z", &parent, sha]) {
        Ok(out) => out,
        Err(_) => git_output(
            repo,
            &[
                "diff-tree",
                "--root",
                "--no-commit-id",
                "--name-only",
                "-r",
                "-z",
                sha,
            ],
        )?,
    };
    Ok(out
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect())
}

fn build_commit(repo: &Path, raw: RawCommit, zero_major: bool) -> anyhow::Result<Commit> {
    let parsed = parse_subject(&raw.subject);
    let breaking = parsed.breaking || body_breaking(&raw.body);
    let ids = requirement_ids(&raw.subject);
    let body_ids = requirement_ids(&raw.body)
        .difference(&ids)
        .cloned()
        .collect();
    let level = level_of(&parsed, breaking, zero_major);
    let files = changed_files(repo, &raw.sha)?;
    Ok(Commit {
        short: raw.sha.chars().take(7).collect(),
        sha: raw.sha,
        date: raw.date,
        subject: raw.subject,
        visible: user_visible(&parsed, breaking),
        parsed,
        breaking,
        ids,
        body_ids,
        files,
        level,
        body: raw.body,
    })
}

// ── release-plan ────────────────────────────────────────────────────────────────────────────

struct PlanOptions {
    since: Option<String>,
    to: String,
    bump: Option<String>,
    bodies: bool,
}

/// `cargo xtask release-plan [--since <tag>] [--to <rev>] [--bump <level|version>] [--bodies]`
pub fn run_release_plan(args: &[String]) -> anyhow::Result<()> {
    let parsed = Args::new(args);
    parsed.reject_unknown(&["--since", "--to", "--bump"], &["--bodies"])?;
    let options = PlanOptions {
        since: parsed.value("--since")?.map(str::to_owned),
        to: parsed.value("--to")?.unwrap_or("HEAD").to_owned(),
        bump: parsed.value("--bump")?.map(str::to_owned),
        bodies: parsed.flag("--bodies"),
    };
    let plan = plan_json(&workspace_root(), &options)?;
    println!("{}", serde_json::to_string_pretty(&plan)?);
    Ok(())
}

fn plan_json(repo: &Path, options: &PlanOptions) -> anyhow::Result<Value> {
    validate_rev("--to", &options.to)?;
    if let Some(since) = &options.since {
        validate_rev("--since", since)?;
    }
    let commit_of = |rev: &str| {
        git_output(
            repo,
            &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
        )
        .map(|sha| sha.trim().to_owned())
    };
    let to = commit_of(&options.to)?;

    let tags = version_tags(repo)?;
    let last = tags.last().cloned();
    let stable = tags.iter().rev().find(|(_, v)| v.pre.is_empty()).cloned();
    let (base_tag, base) = match &options.since {
        Some(since) => {
            commit_of(since)?;
            let version = since
                .strip_prefix('v')
                .and_then(|v| Version::parse(v).ok())
                .or_else(|| stable.as_ref().map(|(_, v)| v.clone()));
            (Some(since.clone()), version)
        }
        None => (
            stable.as_ref().map(|(tag, _)| tag.clone()),
            stable.as_ref().map(|(_, v)| v.clone()),
        ),
    };
    let range = match &base_tag {
        Some(tag) => format!("{tag}..{to}"),
        None => to.clone(),
    };
    let zero_major = base.as_ref().is_none_or(|v| v.major == 0);
    let commits = read_commits(repo, &range)?
        .into_iter()
        .map(|raw| build_commit(repo, raw, zero_major))
        .collect::<anyhow::Result<Vec<_>>>()?;

    let manifest = git_output(repo, &["show", &format!("{to}:Cargo.toml")])?;
    let workspace = workspace_version_of(&manifest);
    let workspace_semver = workspace.as_deref().and_then(|v| Version::parse(v).ok());
    let decision = decide(
        base_tag.as_deref(),
        base.as_ref(),
        last.as_ref().map(|(_, v)| v),
        workspace_semver.as_ref(),
        &commits,
        options.bump.as_deref(),
    )?;

    let commits: Vec<Value> = commits
        .iter()
        .map(|c| commit_json(c, options.bodies))
        .collect();
    let mut plan = json!({
        "base_tag": base_tag,
        "last_tag": last.as_ref().map(|(tag, _)| tag),
        "last_version": last.as_ref().map(|(_, v)| v.to_string()),
        "to": to,
        "workspace_version": workspace,
        "commits": commits,
        "bump": decision.bump.as_str(),
        "next_version": decision.next.as_ref().map(Version::to_string),
        "reason": decision.reason,
        "internal_only": decision.internal_only,
        "warnings": decision.warnings,
    });
    if let Some(request) = &options.bump {
        plan["requested"] = json!(request);
    }
    Ok(plan)
}

/// Spec files a commit changed. The spec changelog is the output of the release step, not a change.
fn spec_files(files: &[String]) -> Vec<&String> {
    files
        .iter()
        .filter(|f| f.starts_with("docs/spec/") && *f != "docs/spec/CHANGELOG.md")
        .collect()
}

fn commit_json(c: &Commit, bodies: bool) -> Value {
    let spec_files = spec_files(&c.files);
    let mut value = json!({
        "sha": c.sha,
        "short": c.short,
        "date": c.date,
        "subject": c.subject,
        "type": c.parsed.kind,
        "scope": c.parsed.scope,
        "breaking": c.breaking,
        "description": c.parsed.description,
        "pr": c.parsed.pr,
        "ids": c.ids,
        "body_ids": c.body_ids,
        "files": c.files,
        "spec_files": spec_files,
        "user_visible": c.visible,
        "level": c.level.as_str(),
    });
    if bodies {
        value["body"] = json!(c.body);
    }
    value
}

// ── release-verify ──────────────────────────────────────────────────────────────────────────

type Check = Result<String, String>;

/// `cargo xtask release-verify --version <semver> [--remote] [--allow-existing-tag]`
pub fn run_release_verify(args: &[String]) -> anyhow::Result<()> {
    let parsed = Args::new(args);
    parsed.reject_unknown(&["--version"], &["--remote", "--allow-existing-tag"])?;
    let version = parsed
        .value("--version")?
        .context("--version is required")?;
    let version = version.strip_prefix('v').unwrap_or(version);
    let allow_tag = parsed.flag("--allow-existing-tag");
    let root = workspace_root();

    let members = workspace_members()?;
    let names: Vec<String> = members.iter().map(|(name, _)| name.clone()).collect();
    let lock = fs::read_to_string(root.join("Cargo.lock")).context("reading Cargo.lock")?;
    let changelog =
        fs::read_to_string(root.join("CHANGELOG.md")).context("reading CHANGELOG.md")?;

    let mut checks: Vec<(&str, Check)> = vec![
        ("version-syntax", check_version_syntax(version)),
        (
            "workspace-version",
            check_member_versions(&members, version),
        ),
        ("lockfile", check_lockfile(&lock, &names, version)),
        ("changelog", check_changelog(&changelog, version)),
    ];
    if allow_tag {
        eprintln!("xtask: --allow-existing-tag: skipping the tag checks");
    } else {
        let tags = version_tags(&root)?;
        let remote = if parsed.flag("--remote") {
            let refname = format!("refs/tags/v{version}");
            Some(
                !git_output(&root, &["ls-remote", "--tags", "origin", &refname])?
                    .trim()
                    .is_empty(),
            )
        } else {
            None
        };
        checks.push(("tag-absent", check_tag_absent(&tags, version, remote)));
        checks.push(("version-order", check_version_order(&tags, version)));
    }

    let mut failed = 0;
    for (name, check) in &checks {
        match check {
            Ok(detail) => println!("ok   {name}: {detail}"),
            Err(detail) => {
                failed += 1;
                println!("FAIL {name}: {detail}");
            }
        }
    }
    if failed > 0 {
        bail!("{failed} release check(s) failed");
    }
    Ok(())
}

fn workspace_members() -> anyhow::Result<Vec<(String, String)>> {
    let json = output(cargo().args(["metadata", "--no-deps", "--format-version", "1"]))?;
    let value: Value = serde_json::from_str(&json).context("parsing cargo metadata")?;
    value["packages"]
        .as_array()
        .context("cargo metadata has no packages")?
        .iter()
        .map(|p| {
            Some((
                p["name"].as_str()?.to_owned(),
                p["version"].as_str()?.to_owned(),
            ))
        })
        .collect::<Option<Vec<_>>>()
        .context("malformed cargo metadata")
}

fn check_version_syntax(version: &str) -> Check {
    validate_version(version).map_err(|e| e.to_string())?;
    Version::parse(version).map_err(|e| e.to_string())?;
    Ok(version.to_owned())
}

fn check_member_versions(members: &[(String, String)], version: &str) -> Check {
    let wrong: Vec<String> = members
        .iter()
        .filter(|(_, v)| v != version)
        .map(|(name, v)| format!("{name} is {v}"))
        .collect();
    if wrong.is_empty() {
        Ok(format!(
            "all {} workspace crates are {version}",
            members.len()
        ))
    } else {
        Err(format!("expected {version}: {}", wrong.join(", ")))
    }
}

struct LockEntry {
    name: String,
    version: String,
    has_source: bool,
}

fn lock_entries(lock: &str) -> Vec<LockEntry> {
    let mut entries: Vec<LockEntry> = Vec::new();
    for line in lock.lines() {
        if line == "[[package]]" {
            entries.push(LockEntry {
                name: String::new(),
                version: String::new(),
                has_source: false,
            });
        } else if let Some(entry) = entries.last_mut() {
            let value = |key: &str| {
                line.strip_prefix(key)
                    .and_then(|rest| rest.trim_start().strip_prefix('='))
                    .map(|v| v.trim().trim_matches('"').to_owned())
            };
            if let Some(name) = value("name") {
                entry.name = name;
            } else if let Some(version) = value("version") {
                entry.version = version;
            } else if line.starts_with("source") {
                entry.has_source = true;
            }
        }
    }
    entries
}

/// Workspace crates are path packages (no `source`); each must be locked at `version`.
fn check_lockfile(lock: &str, members: &[String], version: &str) -> Check {
    let entries = lock_entries(lock);
    let mut problems = Vec::new();
    for member in members {
        let own: Vec<&LockEntry> = entries
            .iter()
            .filter(|e| e.name == *member && !e.has_source)
            .collect();
        match own.as_slice() {
            [entry] if entry.version == version => {}
            [entry] => problems.push(format!("{member} is {} in Cargo.lock", entry.version)),
            [] => problems.push(format!("{member} is missing from Cargo.lock")),
            _ => problems.push(format!("{member} appears more than once in Cargo.lock")),
        }
    }
    if problems.is_empty() {
        Ok(format!("{} workspace crates are {version}", members.len()))
    } else {
        Err(format!(
            "{} (run `cargo update --workspace`)",
            problems.join(", ")
        ))
    }
}

/// The heading `release.yml` looks for, a valid date if there is one, and a nonblank section.
fn check_changelog(text: &str, version: &str) -> Check {
    let heading = format!("## [{version}]");
    let dated = format!("{heading} - ");
    let mut lines = text.lines();
    let Some(found) = lines
        .by_ref()
        .find(|l| *l == heading || l.starts_with(&dated))
    else {
        return Err(format!("CHANGELOG.md has no '{heading}' section"));
    };
    if let Some(rest) = found.strip_prefix(&dated) {
        let date = rest.split_whitespace().next().unwrap_or("");
        if Date::parse(date, format_description!("[year]-[month]-[day]")).is_err() {
            return Err(format!(
                "the date `{date}` in '{found}' is not a valid YYYY-MM-DD date"
            ));
        }
    }
    let blank = !lines
        .take_while(|l| !l.starts_with("## ["))
        .any(|l| !l.trim().is_empty());
    if blank {
        return Err(format!("the '{heading}' section is empty"));
    }
    Ok(found.to_owned())
}

fn check_tag_absent(tags: &[(String, Version)], version: &str, remote: Option<bool>) -> Check {
    let name = format!("v{version}");
    if tags.iter().any(|(tag, _)| *tag == name) {
        return Err(format!("the tag {name} already exists locally"));
    }
    match remote {
        Some(true) => Err(format!("the tag {name} already exists on origin")),
        Some(false) => Ok(format!("{name} exists neither locally nor on origin")),
        None => Ok(format!(
            "{name} does not exist locally (origin not checked)"
        )),
    }
}

fn check_version_order(tags: &[(String, Version)], version: &str) -> Check {
    let version = Version::parse(version).map_err(|e| e.to_string())?;
    match tags.last() {
        Some((tag, last)) if version <= *last => Err(format!(
            "{version} is not greater than the latest tag {tag}"
        )),
        Some((tag, _)) => Ok(format!("{version} is greater than {tag}")),
        None => Ok("no earlier release tag".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn commit(subject: &str, body: &str, zero_major: bool) -> Commit {
        let parsed = parse_subject(subject);
        let breaking = parsed.breaking || body_breaking(body);
        Commit {
            sha: "0123456789abcdef".to_owned(),
            short: "0123456".to_owned(),
            date: "2026-10-06".to_owned(),
            subject: subject.to_owned(),
            visible: user_visible(&parsed, breaking),
            level: level_of(&parsed, breaking, zero_major),
            parsed,
            breaking,
            ids: requirement_ids(subject),
            body_ids: BTreeSet::new(),
            files: Vec::new(),
            body: body.to_owned(),
        }
    }

    #[test]
    fn rel_018_subject_parsing() {
        let s = parse_subject("fix(windows): hide release console window [REL-028] (#30)");
        assert_eq!(s.kind, "fix");
        assert_eq!(s.scope.as_deref(), Some("windows"));
        assert!(!s.breaking);
        assert_eq!(s.description, "hide release console window [REL-028]");
        assert_eq!(s.pr, Some(30));

        let s = parse_subject("feat(ui)!: drop the old sidebar (#7)");
        assert_eq!((s.kind.as_str(), s.breaking, s.pr), ("feat", true, Some(7)));

        let s = parse_subject("chore: tidy");
        assert_eq!((s.scope, s.pr), (None, None));

        for text in [
            "Merge pull request #12 from a/b",
            "Fix: capital",
            "feat:no space",
            "x(): y",
            "(): z",
        ] {
            let s = parse_subject(text);
            assert_eq!(s.kind, "other", "{text}");
            assert_eq!(s.description, text.trim_end());
        }
        assert_eq!(parse_subject("fix: a (#x)").pr, None);
        assert_eq!(parse_subject("fix: a (#)").pr, None);
    }

    #[test]
    fn rel_018_requirement_ids() {
        let ids = requirement_ids(
            "feat(ui): x [REL-016, REL-017] and ENG-108/ENG-110, SHA-256, XREL-001, REL-0010, UTF-8",
        );
        assert_eq!(
            ids.into_iter().collect::<Vec<_>>(),
            ["ENG-108", "ENG-110", "REL-016", "REL-017"]
        );
        assert!(requirement_ids("").is_empty());
        assert!(requirement_ids("REL-01").is_empty());
        assert_eq!(requirement_ids("REL-010…013").len(), 1);
    }

    #[test]
    fn rel_018_classification_and_levels() {
        let cases = [
            ("feat(ui): name engines", Level::Minor),
            ("fix(wslc): repair", Level::Patch),
            ("perf(core): faster", Level::Patch),
            ("refactor(hub): split", Level::Patch),
            ("revert: x", Level::Patch),
            ("Merge branch x", Level::Patch),
            ("docs(release): record", Level::None),
            ("test(wslc): gate", Level::None),
            ("chore(deps): bump", Level::None),
            ("fix(ci): retry macOS DMG packaging", Level::None),
            ("feat(release): skill", Level::None),
            ("fix(ci,ui): both", Level::Patch),
            ("ci: lint", Level::None),
            ("chore!: drop a flag", Level::Minor),
        ];
        for (subject, level) in cases {
            assert_eq!(commit(subject, "", true).level, level, "{subject}");
        }
        assert_eq!(commit("chore!: x", "", false).level, Level::Major);
        assert_eq!(
            commit("fix: x", "details\n\nBREAKING CHANGE: gone", true).level,
            Level::Minor
        );
        assert_eq!(
            commit("fix: x", "BREAKING-CHANGE: gone", false).level,
            Level::Major
        );
    }

    #[test]
    fn rel_018_next_version_and_bump() {
        let (stable, rc) = (v("0.2.0"), v("0.3.0-rc.1"));
        assert_eq!(
            next_version(Some(&stable), Some(&stable), None, Level::Minor),
            Some(v("0.3.0"))
        );
        assert_eq!(
            next_version(Some(&stable), Some(&stable), None, Level::Patch),
            Some(v("0.2.1"))
        );
        assert_eq!(
            next_version(Some(&v("1.4.2")), None, None, Level::Major),
            Some(v("2.0.0"))
        );
        assert_eq!(
            next_version(Some(&stable), Some(&stable), None, Level::None),
            None
        );
        // a pending pre-release of a higher version is finalised, not skipped
        assert_eq!(
            next_version(Some(&stable), Some(&rc), None, Level::Minor),
            Some(v("0.3.0"))
        );
        assert_eq!(
            next_version(Some(&stable), Some(&rc), None, Level::Patch),
            Some(v("0.3.0"))
        );
        assert_eq!(
            next_version(Some(&stable), Some(&rc), None, Level::Major),
            Some(v("1.0.0"))
        );
        // the first release is the version already in Cargo.toml
        assert_eq!(
            next_version(None, None, Some(&v("0.1.0")), Level::Minor),
            Some(v("0.1.0"))
        );
        assert_eq!(next_version(None, None, None, Level::Minor), None);
    }

    #[test]
    fn rel_018_decide_picks_the_highest_level() {
        let commits = [
            commit("fix(ui): a (#1)", "", true),
            commit("feat(ui): b (#2)", "", true),
            commit("docs(spec): c (#3)", "", true),
        ];
        let base = v("0.2.0");
        let d = decide(
            Some("v0.2.0"),
            Some(&base),
            Some(&base),
            None,
            &commits,
            None,
        )
        .unwrap();
        assert_eq!((d.bump, d.next), (Level::Minor, Some(v("0.3.0"))));
        assert!(
            d.reason.starts_with("minor: 0123456 feat(ui): b"),
            "{}",
            d.reason
        );
        assert!(!d.internal_only && d.warnings.is_empty());

        let docs = [commit("docs: x", "", true), commit("ci: y", "", true)];
        let d = decide(Some("v0.2.0"), Some(&base), Some(&base), None, &docs, None).unwrap();
        assert_eq!((d.bump, d.next, d.internal_only), (Level::None, None, true));

        let d = decide(Some("v0.2.0"), Some(&base), Some(&base), None, &[], None).unwrap();
        assert_eq!((d.bump, d.internal_only), (Level::None, false));
        assert_eq!(d.reason, "no commits since v0.2.0");
    }

    #[test]
    fn rel_018_decide_warns_about_other_commits_and_replays() {
        let commits = [
            commit("Merge branch 'x'", "", true),
            commit("feat: y", "", true),
        ];
        let (base, last) = (v("0.1.0"), v("0.2.0"));
        let d = decide(
            Some("v0.1.0"),
            Some(&base),
            Some(&last),
            None,
            &commits,
            None,
        )
        .unwrap();
        assert_eq!(d.next, Some(v("0.2.0")));
        assert_eq!(d.warnings.len(), 2, "{:?}", d.warnings);
        assert!(
            d.warnings
                .iter()
                .any(|w| w.contains("not greater than the latest tag v0.2.0"))
        );
        assert!(
            d.warnings
                .iter()
                .any(|w| w.contains("not conventional commits"))
        );
    }

    #[test]
    fn rel_018_requested_version() {
        let (base, last) = (v("0.2.0"), v("0.2.0"));
        let ask = |r: &str| requested_version(r, Some(&base), Some(&last));
        assert_eq!(ask("patch").unwrap(), v("0.2.1"));
        assert_eq!(ask("minor").unwrap(), v("0.3.0"));
        assert_eq!(ask("0.5.0").unwrap(), v("0.5.0"));
        assert_eq!(ask("v0.3.0-rc.1").unwrap(), v("0.3.0-rc.1"));
        assert_eq!(ask("1.0.0").unwrap(), v("1.0.0"));
        assert!(ask("major").unwrap_err().to_string().contains("1.0.0"));
        assert!(ask("0.2.0").is_err());
        assert!(ask("0.1.9").is_err());
        assert!(ask("0.3").is_err());
        assert!(ask("0.3.0+meta").is_err());
        assert!(ask("0.3.0;rm").is_err());
        assert!(requested_version("patch", None, None).is_err());
        assert_eq!(
            requested_version("major", Some(&v("1.2.3")), None).unwrap(),
            v("2.0.0")
        );
    }

    #[test]
    fn rel_018_requested_version_warnings_and_empty_range() {
        let base = v("0.2.0");
        let docs = [commit("docs: x", "", true)];
        let d = decide(
            Some("v0.2.0"),
            Some(&base),
            Some(&base),
            None,
            &docs,
            Some("patch"),
        )
        .unwrap();
        assert_eq!(d.next, Some(v("0.2.1")));
        assert!(
            d.warnings[0].contains("none user-visible"),
            "{:?}",
            d.warnings
        );

        let feat = [commit("feat: x", "", true)];
        let d = decide(
            Some("v0.2.0"),
            Some(&base),
            Some(&base),
            None,
            &feat,
            Some("patch"),
        )
        .unwrap();
        assert!(
            d.warnings[0].contains("lower than the computed minor bump"),
            "{:?}",
            d.warnings
        );

        let err = decide(
            Some("v0.2.0"),
            Some(&base),
            Some(&base),
            None,
            &[],
            Some("patch"),
        );
        assert!(
            err.err()
                .unwrap()
                .to_string()
                .contains("nothing to release")
        );
    }

    #[test]
    fn rel_019_spec_files_exclude_the_spec_changelog() {
        let files: Vec<String> = [
            "docs/spec/CHANGELOG.md",
            "docs/spec/features/distribution.md",
            "docs/spec/README.md",
            "docs/plan/features/x.md",
            "crates/dockering/src/main.rs",
            "docs/spec/features/CHANGELOG.md",
        ]
        .map(str::to_owned)
        .to_vec();
        let kept: Vec<&str> = spec_files(&files).into_iter().map(String::as_str).collect();
        assert_eq!(
            kept,
            [
                "docs/spec/features/distribution.md",
                "docs/spec/README.md",
                "docs/spec/features/CHANGELOG.md"
            ]
        );
        assert!(spec_files(&["docs/spec/CHANGELOG.md".to_owned()]).is_empty());
    }

    #[test]
    fn rel_018_workspace_version_of_manifest() {
        let manifest = "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"0.2.0\"\nedition = \"2024\"\n\n[workspace.dependencies]\nsemver = { version = \"1\" }\n";
        assert_eq!(workspace_version_of(manifest).as_deref(), Some("0.2.0"));
        assert_eq!(
            workspace_version_of("[package]\nversion = \"9.9.9\"\n"),
            None
        );
    }

    #[test]
    fn rel_018_rev_validation() {
        for ok in ["HEAD", "origin/main", "v0.2.0", "HEAD~3", "abc123^"] {
            assert!(validate_rev("--to", ok).is_ok(), "{ok}");
        }
        for bad in ["", "-x", "--output=x", "a..b", "a b", "a;b", "a$(b)"] {
            assert!(validate_rev("--to", bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn rel_018_utc_dates() {
        assert_eq!(utc_date("2026-10-06T02:16:47+02:00").unwrap(), "2026-10-06");
        assert_eq!(utc_date("2026-10-06T01:30:00+02:00").unwrap(), "2026-10-05");
        assert!(utc_date("yesterday").is_err());
    }

    #[test]
    fn rel_018_changelog_heading_and_body() {
        let text = "# Changelog\n\n## [0.3.0] - 2026-10-06\n\n### Added\n\n- x\n\n## [0.2.0] - 2026-10-05\n\n- y\n";
        assert!(check_changelog(text, "0.3.0").is_ok());
        assert!(check_changelog(text, "0.2.0").is_ok());
        assert!(
            check_changelog(text, "0.2")
                .unwrap_err()
                .contains("no '## [0.2]'")
        );
        assert!(check_changelog(text, "0.4.0").is_err());
        // dots in the version are literal
        assert!(check_changelog("## [0x2x0]\n- a\n", "0.2.0").is_err());
        // no date is accepted, a bad date is not
        assert!(check_changelog("## [1.0.0]\n\n- a\n", "1.0.0").is_ok());
        assert!(
            check_changelog("## [1.0.0] - 2026-13-45\n\n- a\n", "1.0.0")
                .unwrap_err()
                .contains("not a valid")
        );
        // an empty section is rejected, as the notes would be empty
        assert!(
            check_changelog(
                "## [1.0.0] - 2026-10-06\n\n\n## [0.9.0] - 2026-10-01\n- a\n",
                "1.0.0"
            )
            .unwrap_err()
            .contains("empty")
        );
        assert!(check_changelog("## [1.0.0] - 2026-10-06\n", "1.0.0").is_err());
    }

    #[test]
    fn rel_018_lockfile_versions() {
        let lock = "version = 4\n\n[[package]]\nname = \"dk-core\"\nversion = \"0.3.0\"\n\n[[package]]\nname = \"dk-hub\"\nversion = \"0.2.0\"\n\n[[package]]\nname = \"gpui-pre-windows\"\nversion = \"0.3.7\"\n\n[[package]]\nname = \"semver\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n[[package]]\nname = \"dk-update\"\nversion = \"0.3.0\"\nsource = \"registry+https://example.test\"\n";
        let members = ["dk-core".to_owned()];
        assert!(check_lockfile(lock, &members, "0.3.0").is_ok());
        let err = check_lockfile(lock, &["dk-core".into(), "dk-hub".into()], "0.3.0").unwrap_err();
        assert!(
            err.contains("dk-hub is 0.2.0") && !err.contains("dk-core"),
            "{err}"
        );
        let err = check_lockfile(lock, &["dk-missing".into()], "0.3.0").unwrap_err();
        assert!(err.contains("missing"), "{err}");
        // a registry package of the same name is not the workspace crate
        let err = check_lockfile(lock, &["dk-update".into()], "0.3.0").unwrap_err();
        assert!(err.contains("missing"), "{err}");
        // crates that aren't members are not checked
        assert!(check_lockfile(lock, &[], "9.9.9").is_ok());
    }

    #[test]
    fn rel_018_member_versions_and_tags() {
        let members = [
            ("a".to_owned(), "0.3.0".to_owned()),
            ("b".to_owned(), "0.2.0".to_owned()),
        ];
        assert!(
            check_member_versions(&members, "0.3.0")
                .unwrap_err()
                .contains("b is 0.2.0")
        );
        assert!(check_member_versions(&members[..1], "0.3.0").is_ok());

        let tags = [
            ("v0.1.0".to_owned(), v("0.1.0")),
            ("v0.2.0".to_owned(), v("0.2.0")),
        ];
        assert!(check_tag_absent(&tags, "0.3.0", None).is_ok());
        assert!(check_tag_absent(&tags, "0.2.0", None).is_err());
        assert!(
            check_tag_absent(&tags, "0.3.0", Some(true))
                .unwrap_err()
                .contains("origin")
        );
        assert!(check_tag_absent(&tags, "0.3.0", Some(false)).is_ok());
        assert!(check_version_order(&tags, "0.3.0").is_ok());
        assert!(check_version_order(&tags, "0.3.0-rc.1").is_ok());
        assert!(check_version_order(&tags, "0.2.0").is_err());
        assert!(check_version_order(&tags, "0.1.5").is_err());
        assert!(check_version_order(&[], "0.1.0").is_ok());
        assert!(check_version_syntax("0.3.0-rc.1").is_ok());
        assert!(check_version_syntax("0.3").is_err());
    }

    struct TempRepo(PathBuf);

    impl TempRepo {
        fn new() -> Option<Self> {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "xtask-plan-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&dir).ok()?;
            let repo = Self(dir);
            repo.git(&["init", "-q", "-b", "main"]).ok()?;
            repo.write("Cargo.toml", "[workspace.package]\nversion = \"0.1.0\"\n");
            Some(repo)
        }

        fn git(&self, args: &[&str]) -> anyhow::Result<String> {
            let mut cmd = Command::new("git");
            cmd.args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
            ])
            .args(args)
            .current_dir(&self.0);
            output(&mut cmd)
        }

        fn write(&self, path: &str, text: &str) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        fn commit(&self, subject: &str, body: &str, path: &str) {
            self.write(path, subject);
            self.git(&["add", "-A"]).unwrap();
            self.git(&["commit", "-q", "-m", subject, "-m", body])
                .unwrap();
        }

        fn tag(&self, name: &str) {
            self.git(&["tag", "-a", name, "-m", name]).unwrap();
        }

        fn plan(&self, since: Option<&str>, bump: Option<&str>) -> anyhow::Result<Value> {
            plan_json(
                &self.0,
                &PlanOptions {
                    since: since.map(str::to_owned),
                    to: "HEAD".to_owned(),
                    bump: bump.map(str::to_owned),
                    bodies: true,
                },
            )
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn repo_or_skip() -> Option<TempRepo> {
        let repo = TempRepo::new();
        if repo.is_none() {
            eprintln!("git is unavailable; skipping");
        }
        repo
    }

    #[test]
    fn rel_018_plan_over_a_git_history() {
        let Some(repo) = repo_or_skip() else { return };
        repo.commit("feat: first (#1)", "", "a.txt");
        repo.tag("v0.1.0");
        repo.commit(
            "fix(ui): crash on start [KBD-001] (#2)",
            "Also fixes SHL-002.",
            "b.txt",
        );
        repo.commit("docs(spec): notes [REL-011] (#3)", "", "docs/spec/x.md");
        repo.commit("feat(ui)!: new sidebar (#4)", "", "c.txt");

        let plan = repo.plan(None, None).unwrap();
        assert_eq!(plan["base_tag"], "v0.1.0");
        assert_eq!(plan["last_version"], "0.1.0");
        assert_eq!(plan["workspace_version"], "0.1.0");
        assert_eq!(
            (plan["bump"].as_str(), plan["next_version"].as_str()),
            (Some("minor"), Some("0.2.0"))
        );
        let commits = plan["commits"].as_array().unwrap();
        assert_eq!(commits.len(), 3, "oldest first, since the tag only");
        assert_eq!(commits[0]["pr"], 2);
        assert_eq!(commits[0]["ids"], json!(["KBD-001"]));
        assert_eq!(commits[0]["body_ids"], json!(["SHL-002"]));
        assert_eq!(commits[0]["files"], json!(["b.txt"]));
        assert_eq!(commits[0]["level"], "patch");
        assert_eq!(commits[1]["user_visible"], false);
        assert_eq!(commits[1]["spec_files"], json!(["docs/spec/x.md"]));
        assert_eq!(commits[2]["breaking"], true);
        assert_eq!(commits[2]["level"], "minor");
        assert!(commits[0]["date"].as_str().unwrap().len() == 10);
        assert_eq!(commits[0]["body"], "Also fixes SHL-002.");
        assert!(plan.get("requested").is_none());
        assert!(plan["warnings"].as_array().unwrap().is_empty());
    }

    #[test]
    fn rel_018_plan_with_only_internal_commits() {
        let Some(repo) = repo_or_skip() else { return };
        repo.commit("feat: first", "", "a.txt");
        repo.tag("v0.1.0");
        repo.commit("docs(release): record v0.1.0 (#5)", "", "docs/r.md");
        repo.commit("ci: tweak", "", ".github/x.yml");
        let plan = repo.plan(None, None).unwrap();
        assert_eq!(
            (plan["bump"].as_str(), plan["internal_only"].as_bool()),
            (Some("none"), Some(true))
        );
        assert!(plan["next_version"].is_null());
        assert!(
            repo.plan(None, Some("0.3.0")).unwrap()["warnings"][0]
                .as_str()
                .unwrap()
                .contains("none user-visible")
        );
        assert_eq!(
            repo.plan(None, Some("patch")).unwrap()["next_version"],
            "0.1.1"
        );
    }

    #[test]
    fn rel_018_plan_without_commits_or_tags() {
        let Some(repo) = repo_or_skip() else { return };
        repo.commit("feat: first", "", "a.txt");
        let plan = repo.plan(None, None).unwrap();
        assert!(plan["base_tag"].is_null() && plan["last_tag"].is_null());
        assert_eq!(
            (plan["bump"].as_str(), plan["next_version"].as_str()),
            (Some("minor"), Some("0.1.0"))
        );
        repo.tag("v0.1.0");
        assert_eq!(repo.plan(None, None).unwrap()["bump"], "none");
        assert!(
            repo.plan(None, Some("patch"))
                .unwrap_err()
                .to_string()
                .contains("nothing to release")
        );
    }

    #[test]
    fn rel_018_plan_since_an_older_tag_and_prereleases() {
        let Some(repo) = repo_or_skip() else { return };
        repo.commit("feat: first", "", "a.txt");
        repo.tag("v0.1.0");
        repo.commit("feat(ui): second", "", "b.txt");
        repo.tag("v0.2.0");
        repo.commit("fix: third", "", "c.txt");
        repo.tag("v0.3.0-rc.1");
        repo.commit("fix: fourth", "", "d.txt");

        let plan = repo.plan(None, None).unwrap();
        assert_eq!(plan["base_tag"], "v0.2.0");
        assert_eq!(plan["last_tag"], "v0.3.0-rc.1");
        assert_eq!(
            plan["next_version"], "0.3.0",
            "the pending pre-release is finalised"
        );
        assert_eq!(plan["commits"].as_array().unwrap().len(), 2);

        let replay = repo.plan(Some("v0.1.0"), None).unwrap();
        assert_eq!(replay["commits"].as_array().unwrap().len(), 3);
        assert_eq!(replay["bump"], "minor");
        assert_eq!(replay["next_version"], "0.3.0");
        assert!(replay["warnings"].as_array().unwrap().is_empty());
        assert!(repo.plan(Some("v0.1.0"), Some("0.2.0")).is_err());
        assert_eq!(
            repo.plan(None, Some("0.3.0")).unwrap()["requested"],
            "0.3.0"
        );
        assert!(repo.plan(Some("nope"), None).is_err());
    }

    #[test]
    fn rel_018_plan_breaking_change_at_one_point_zero() {
        let Some(repo) = repo_or_skip() else { return };
        repo.commit("feat: first", "", "a.txt");
        repo.tag("v1.0.0");
        repo.commit("fix: x", "BREAKING CHANGE: the config file moved", "b.txt");
        let plan = repo.plan(None, None).unwrap();
        assert_eq!(
            (plan["bump"].as_str(), plan["next_version"].as_str()),
            (Some("major"), Some("2.0.0"))
        );
    }
}
