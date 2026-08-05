# WorkTable beta.5 persistence compatibility

This campaign verifies that WorkTable `1.0.0-beta.5` can open and validate a
persisted table written by every WorkTable version found in the sibling service
repositories during the beta.5 port:

- `0.8.19`
- `0.9.0-beta0.2.2`
- `0.9.0-beta0.2.3`
- `0.9.2`
- `0.9.4`

Each writer is compiled as a separate temporary Cargo project because the old
WorkTable releases pin mutually incompatible exact DataBucket proc-macro
versions. The common table contains a string primary key, an indexed integer,
and an unsized string payload. The beta.5 reader verifies the row count, every
primary-key lookup, every payload, and the secondary-index result.

Run from any directory:

```sh
campaigns/persistence-compat/run.sh
```

This proves compatibility for the exercised schema and operations. A deployment
must still stop writers and take a recoverable copy of its real store before the
first beta.5 startup; this campaign is not a substitute for a production backup.
