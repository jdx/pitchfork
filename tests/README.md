## Tests

### Why use bun

Bun supports a shell-like feature [`$ Shell`](https://bun.sh/docs/runtime/shell), which is compatible with Windows and gives you excellent readability.

And bun just needs a `bun run` to run. Note that if you do not write tests, `bun install` is not needed, because all dependencies are dev-only.

### Tests TODO

- [x] Why `pf start` takes ~700ms? (not `--release`)
- [x] Cron
- [x] More complex situations
- [ ] Behaviour when `retry_delay` & `retry_output` exist together, the test needs to be modified

### Cron tests

Cron tests cost a long time (~2min each task, and not sure if they can run in parallel), because the interval of `cron_watch` is 1min. And cron is an individual module of the project, so #test_e2e_cron is ignored by default. Run `cargo test -- --ignored` to run it.
