# Agent notes for omnimodem

Guidance for coding agents working in this repo.

## Setting a GitHub PR/issue body from a file (gh)

Do NOT use `gh api -f body=@file` (`-f`/`--raw-field`). `-f` writes the value as a
literal string and does not read files, so the body becomes the literal text
`@path/to/file` — with no error. (This clobbered a PR description once.) The
file-reading `@` only works with the capital `-F`/`--field` flag.

Prefer, in order:

1. `gh api -X PATCH <endpoint> --input payload.json`, where `payload.json` is built
   with a JSON-escaping tool:
   `python3 -c "import json; open('payload.json','w').write(json.dumps({'body': open('body.md').read()}))"`.
   No `@`-expansion, no shell quoting, handles any markdown.
2. `gh pr edit <n> --body-file <path>` — cleanest, but fails if the token lacks the
   `read:org` scope; fall back to option 1 when it errors.
3. `gh api ... -F body=@file` (capital `-F`) — reads the file, but type-coerces
   values, so slightly riskier for arbitrary text than `--input`.

After writing, read the field back to confirm it isn't a stray `@path` literal:
`gh pr view <n> --json body -q .body | head -1`.
