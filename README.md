# cargo-verdict

Structured results for cargo's build and test verdicts.

- **Request model**: one normalized argv model for build/test requests (`parse` / `render`).
- **Planner**: adds only output and runner flags, and reports which it added.
- **Demux**: cargo messages (via `cargo_metadata`) and libtest events into one versioned
  result schema, `cargo-verdict/1`, with the raw output kept.

Use it as a library (`cargo_verdict`) or as `cargo verdict …`.

## License

Licensed under either of Apache License 2.0 or MIT at your option.
