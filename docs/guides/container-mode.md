---
description: Run the foreground supervisor as a container entrypoint with signal handling and orphan reaping.
---
# Container mode <Badge type="warning" text="Experimental" />

Container mode lets the supervisor act as PID 1 on Linux. It reaps orphaned
children and handles `SIGTERM` and `SIGINT` through graceful shutdown. This
feature is experimental; test the lifecycle of your services in your container.

## Run the supervisor in the foreground

```sh
pitchfork supervisor run --container --boot
```

`--container` enables PID 1 behavior. `--boot` starts configured daemons whose
`boot_start` is `true`. Container mode alone does not start every daemon.

You can also enable container behavior with `PITCHFORK_CONTAINER=true` or
`[settings.supervisor] container = true`.

## A minimal example

This example serves `/app` with Python. Put a Linux pitchfork binary matching
the container's architecture at `./pitchfork`, alongside the Dockerfile and
configuration. You can obtain it from [releases](https://github.com/jdx/pitchfork/releases).

```dockerfile
FROM python:3-slim
COPY --chmod=755 pitchfork /usr/local/bin/pitchfork
WORKDIR /app
COPY pitchfork.toml /app/pitchfork.toml
EXPOSE 8000
ENTRYPOINT ["pitchfork", "supervisor", "run", "--container", "--boot"]
```

`pitchfork.toml`:

```toml
[daemons.web]
run = "python3 -u -m http.server 8000 --bind 0.0.0.0"
ready_port = { port = 8000, timeout = "15s" }
boot_start = true
retry = true
```

```sh
docker build -t pitchfork-demo .
docker run --rm --name pitchfork-demo -p 127.0.0.1:8000:8000 pitchfork-demo
```

Open [http://127.0.0.1:8000](http://127.0.0.1:8000). From another terminal, run
`docker stop pitchfork-demo` to exercise graceful shutdown.

## Use your own application

Install the application's runtime and dependencies in the image, copy its files,
and replace the `run` command. Keep service commands in the foreground and set
`boot_start = true` for entrypoint services. Add `depends` and ready checks when
services must start in order.

The supervisor remains the container's main process. Do not assume a child
daemon's exit will end the container or become the container's exit status.
Inspect daemon [logs](/guides/logs) and status, and configure
[health checks](/guides/health-checks) as needed.
