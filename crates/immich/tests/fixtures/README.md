# Fixtures

Each file says whether it was **captured** from a real server or **constructed** from the schema:

- `*_real.json` — captured from a real Immich 3.2.0 server (scrubbed); see each test's comment.
- `*_constructed.json` — constructed from the 3.2.0 OpenAPI schema; they prove only that the parser agrees with
  the schema, never that a server sends this (M10.18d's album replies).
- The others say which they are in the comment on the test constant that reads them.
