# Shared Claude Code and OpenCode setup

Run either tool from this repository. The repository rules, specialist prompts, and skills
are shared, with small host-specific registrations for OpenCode. No bootstrap, generated
copies, symlinks, or additional packages are needed, including on Windows and in Git worktrees.

## Sources of truth

| Content | Edit here | Claude Code | OpenCode |
|---|---|---|---|
| Repository rules | `AGENTS.md` | Imported by `CLAUDE.md` via `@AGENTS.md` | Discovered natively |
| Specialist agent prompts | `.claude/agents/<name>.md` | Discovered natively | Referenced with `{file:...}` in `opencode.json` |
| Agent metadata / tool access | Claude frontmatter + OpenCode registration | `name`, `description`, `tools` frontmatter | `agent.<name>` in `opencode.json` |
| Skills and supporting files | `.agents/skills/<name>/` | Read through command adapters and shared repository guidance | Discovered natively and explicitly included through `skills.paths` |
| Feature-planning command | Shared `feature-planning` skill | `.claude/commands/feature-planning.md` reads the shared skill | `command.feature-planning` adapter in `opencode.json` loads that skill |

OpenCode reads each complete Claude agent file as prompt text, including its frontmatter.
That frontmatter is context, **not OpenCode configuration**. The OpenCode registration sets
subagent mode, a routing description, and permissions matching the Claude tool list.
The architect can edit only `docs/spec/**` and `docs/plan/**`; the reviewer has no file-edit
tools. As in Claude Code, the reviewer's shell access allows checks to run and is not an
OS-level read-only sandbox. All specialists can load shared skills in OpenCode.

Models and providers use each contributor's host configuration. There are no pinned models,
credentials, or machine-specific paths in the repository configuration.

`.agents/skills/` is a vendor-neutral discovery convention supported natively by OpenCode
and Codex. The skills use the open Agent Skills `SKILL.md` format. Other tools can use the
same files when they support this directory or configurable skill paths. Claude Code's
documented discovery locations still use `.claude/skills/`, so a thin command adapter reads
the canonical skill instead. Shared `AGENTS.md` also tells Claude when to read it for natural
language requests. This preserves one copy of the workflow and its supporting files.

## Use

Both tools support the existing planning workflow:

```text
/feature-planning add container rename
/feature-planning update container-rename adjust validation
/feature-planning complete container-rename
/feature-planning status
```

In Claude Code, ask the main agent to delegate to a specialist, or select one with `/agents`.
In OpenCode, mention the specialist directly, for example:

```text
@architect design the engine contract changes
@rust-core implement the approved core task
@gpui-ui implement the approved UI task
@reviewer review the current diff
```

Available specialists: `architect`, `rust-core`, `engine-integrator`, `windows-platform`,
`gpui-ui`, `qa-engineer`, `reviewer`, and `release-engineer`.

The shared skill uses host-equivalent tools: Claude Code's `AskUserQuestion` corresponds
to OpenCode's `question`, and Claude Code's `Task` corresponds to OpenCode's `task`.

## Maintain

- Change repository instructions in `AGENTS.md`; keep `CLAUDE.md` as the import adapter.
- Edit a specialist's prompt once in `.claude/agents/<name>.md`. OpenCode reads it directly
  on startup, so there is no prompt copy to synchronize.
- When adding or renaming an agent, add or rename its `agent` entry in `opencode.json`.
  Set `mode: subagent`, a useful description, and a `{file:./.claude/agents/<name>.md}` prompt.
  When changing Claude's `tools` list, update the corresponding OpenCode permissions too.
  `Read`/`Grep`/`Glob` map to `read`/`grep`/`glob`; `Edit` and `Write` both map to `edit`;
  `Bash`, `WebFetch`, and `WebSearch` map to `bash`, `webfetch`, and `websearch`.
- Add shared skills under `.agents/skills/<name>/SKILL.md`, with `name` and `description`
  YAML frontmatter. Keep supporting files alongside the skill and use relative links.
  OpenCode and Codex discover new skills without adding a second copy. For Claude Code,
  add a `.claude/commands/<name>.md` adapter with a description that reads the canonical
  `SKILL.md` and forwards `$ARGUMENTS`; add its natural-language trigger to `AGENTS.md`.
  To expose it as an OpenCode slash command, add a command adapter that loads the skill
  and forwards `$ARGUMENTS`, following `feature-planning`.
- Host-specific hooks, MCP server definitions, and local preferences require native host
  configuration. None were present in the repository when this shared setup was added.

## Validate

OpenCode can validate startup and show the resolved registrations without a model call:

```sh
opencode debug config
opencode agent list
opencode debug agent architect
opencode debug agent reviewer
opencode debug skill
```

Check that all eight specialists appear, their prompts contain the shared Markdown, and
`feature-planning` points to the repository's `.agents/skills/feature-planning/SKILL.md`.
`debug config` includes merged user configuration; inspect it locally rather than publishing
its full output. In Claude Code, check `/agents`, `/context`, and `/feature-planning status`.

After editing configuration, agent definitions, or skills, **quit and restart OpenCode**;
running sessions retain already-loaded configuration. Start a fresh Claude Code session
to verify shared instruction changes as well.
