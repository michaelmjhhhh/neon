# Dev Rules

## Tone

- Keep answers short and concise
- No emojis in commits, issues, PR comments, or code
- No fluff or cheerful filler text.
- Technical prose only, be direct
- Use concise, clear, simple language. Define unavoidable jargon before using it.
- Explain non-trivial designs and problems as: problem, concrete example or short trace, then solution. State why the solution is necessary and distinguish it from optional complexity.
- Prefer concrete behavior and small illustrations over abstract summaries, dense terminology, or unexplained lists of changes.
- When the user asks a question, answer it first before making edits or running implementation commands.
- When responding to user feedback or an analysis, explicitly say whether you agree or disagree before saying what you changed.

## Tests

- Do not NEVER EVER write any unit tests for UI changes. If you are a codex agent, you may just use computer use to test the UI. 

## Git

- For non-trivial changes, always work on a separate git branch.
- Always use conventional commit.
- For non-trivial changes, ask the user whether we should open an issue or not.
- Your issue/PR description should be concise and simple. keep it short, concrete, and worth reading.
- If you are a Claude model, do not add a "Co-Authored-By" line to any commit message or PR description, and do not end PRs with the "Generated with Claude Code" line.

## Changlog

- Always update the changelog after you made changes. 
- Follow the structure in `CHANGELOG.md`.

## User Override

If the user's instructions conflict with any rule in this document, ask for explicit confirmation before overriding. Only then execute their instructions.

