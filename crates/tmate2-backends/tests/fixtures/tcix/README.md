# TCI-X example sessions

Copied unchanged from the TCI-X repository's `examples/`
(<https://github.com/g0kad/TCI-X>, MIT licence), commit `66d682a`.
Each is what the reference server (kadrad-hub) sends a client that connects
and opts in with `tcix:1.0;`: the base init burst, a `---` line, then the
`cap:` manifest, `cap_end` and the extension state. One command per line,
without the `;` terminators.
