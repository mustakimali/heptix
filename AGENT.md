# Agent rules

## Changelog

- Keep `CHANGELOG.md` current. Every user-visible change (feature, fix,
  behaviour change, config or CLI change, security hardening) gets an entry
  under `## [Unreleased]` in the same commit that makes the change.
- When bumping `version` in `Cargo.toml`, rename `[Unreleased]` to the new
  version with the date, and add a fresh empty `[Unreleased]` above it. Bumping
  the version on the default branch publishes a release, so do it deliberately.
- Use Keep a Changelog sections: Added, Changed, Fixed, Removed, Security.
- Do not add a changelog entry for pure refactors, tests or docs-only edits.
