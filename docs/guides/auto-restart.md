---
description: Configure bounded or unlimited retries for startup failures and runtime crashes, and understand how they interact with health checks.
---
# Automatic retries

Pitchfork has two independent retry budgets, split by when the failure happens.
`ready_retry` covers failures while a blocking start is still waiting for
readiness. `retry` covers background restarts after a failure.

```toml
[daemons.api]
run = "node server.js"
ready_http = { url = "http://127.0.0.1:3000/health", timeout = "30s" }
ready_retry = 3  # blocking `pitchfork start` retries startup failures 3 times
retry = 5        # the supervisor then restarts it up to 5 times in the background
```

Both budgets apply to termination caused by [health checks](/guides/health-checks)
or resource limits as well.

## Choose a policy

| Value | Behavior |
| --- | --- |
| `0` or `false` | Do not retry (default) |
| A positive integer | Retry up to that many times |
| `true` | Keep retrying on failure until stopped or disabled |

```toml
[daemons.worker]
run = "python3 worker.py"
retry = true
```

Retries respond to failures. A process that completes successfully is not a
crashed service; use [cron](/guides/scheduling) to repeat a successful task.

## Startup failures versus runtime failures

| When it fails | Who retries | Budget | Timing |
| --- | --- | --- | --- |
| Before becoming ready | The blocking start (`pitchfork start` / `run`) | `ready_retry` | Exponential backoff: `1s`, `2s`, `4s`, … |
| After becoming ready | The supervisor in the background | `retry` | Evaluated on interval ticks (`general.interval`, default `10s`) |

During startup retries the CLI keeps waiting until readiness or failure. With
`ready_retry` unset (the default), a first-attempt startup failure returns
immediately and any restarts are left to the `retry` budget. Runtime retries
happen independently of the terminal that started the daemon. Use `depends`
and [ready checks](/guides/ready-checks) to handle startup ordering instead of
relying on failures to delay a dependent service.

The two loops never overlap: while a blocking start is still working (including
its backoff sleeps), the supervisor's background restarts skip that daemon, and
the blocking start stops retrying if the daemon was started elsewhere in the
meantime. An explicit start, a file-watch restart, or a cron trigger resets the
background counter.

## One-off commands

`pitchfork run` accepts both counts:

```sh
pitchfork run worker --retry 3 --ready-retry 2 -- ./worker
```

For `pitchfork start`, configure `ready_retry` and `retry` in `pitchfork.toml`;
it has no retry overrides. You can also save a new daemon with a retry policy:

```sh
pitchfork daemons add worker --run './worker' --retry 3 --ready-retry 2
```

## Inspect or interrupt retries

```sh
pitchfork status api
pitchfork logs api --tail
pitchfork stop api
```

To prevent later manual or automatic starts, use `pitchfork disable api`.
Restore it with `pitchfork enable api`.

Use `on_retry` to react to each retry and `on_fail` when attempts are exhausted.
See [lifecycle hooks](/guides/lifecycle-hooks).
