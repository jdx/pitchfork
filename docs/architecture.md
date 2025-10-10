# Architecture

This document explains how Pitchfork works by following the complete execution flow of the `pitchfork start` command.

## Overview

Pitchfork is a process supervisor built in Rust. The system has two main parts:

1. **CLI** - User-facing commands (start, stop, status, etc.)
2. **Supervisor** - Background daemon that manages all processes

They communicate via Unix domain sockets (IPC).

## The `pitchfork start` Command: A Complete Walkthrough

Let's trace what happens when you run `pitchfork start myapp`.

### Phase 1: CLI Initialization

When a user executes `pitchfork start myapp`, the CLI begins with a series of initialization steps.

First, the CLI reads all `pitchfork.toml` configuration files in the project. Pitchfork supports multiple configuration files - it searches upward from the current directory and merges all found configurations into a unified configuration object via `PitchforkToml::all_merged()`.

Next, the CLI needs to establish a connection with the background supervisor process. It calls `IpcClient::connect(true)` to connect to the supervisor. The `true` parameter means that if the supervisor isn't running, it will automatically start one in the background. This design makes the tool more convenient - users don't need to manually start the supervisor.

After establishing the connection, the CLI queries the supervisor for current state information. It needs to know which daemons are disabled and which are currently running. This information is used for subsequent decisions, such as avoiding duplicate starts of already-running daemons.

Then, the CLI looks up the specified daemon configuration from the merged config. In our example, it searches for the `myapp` daemon configuration, which contains important information like the run command, working directory, autostop options, and retry count.

With this information, the CLI constructs a `RunOptions` struct containing all parameters needed to start the daemon:
- The daemon's ID and command to execute
- Working directory (usually where pitchfork.toml is located)
- Whether autostop functionality is enabled
- Retry count and related retry configuration
- Readiness detection configuration (delay time or output pattern)
- The `wait_ready` flag, indicating whether the command should wait for the daemon to be ready before returning

Finally, the CLI sends this `RunOptions` to the supervisor via IPC and waits for a response.

### Phase 2: IPC Connection

IPC (Inter-Process Communication) is the bridge for communication between the CLI and supervisor. Pitchfork uses Unix domain sockets to implement this communication mechanism.

When the CLI calls `IpcClient::connect()`, it attempts to connect to the Unix socket file located at `~/.local/state/pitchfork/ipc/main.sock`. If the connection fails (e.g., the supervisor isn't running), it starts a new supervisor process.

The connection process uses an exponential backoff strategy. If the first connection attempt fails, it waits 100 milliseconds before retrying; if it fails again, the wait time doubles, with a maximum of 5 retry attempts and a maximum wait time of 1 second. This strategy is necessary because a newly started supervisor needs some time to initialize and create the socket file.

Once connected successfully, the socket is split into read (recv) and write (send) halves. This design allows simultaneous read and write operations without mutual blocking.

The IPC message format is simple yet effective: messages are first serialized into MessagePack binary format (more compact than JSON), then a null byte (`\0`) is appended as a message boundary marker. The receiver reads data until it encounters a null byte, knowing a complete message has been received.

For a `run` request, the CLI constructs an `IpcRequest::Run(opts)` message, serializes it, sends it to the supervisor, and enters a waiting state. This is a typical request-response pattern: send request, block and wait until receiving a response.

The response might be one of several types:
- `DaemonReady` - daemon successfully started and is ready
- `DaemonFailedWithCode` - daemon failed to start, includes exit code
- `DaemonAlreadyRunning` - daemon is already running

### Phase 3: Supervisor Receives Request

The supervisor is a long-running background process whose main job is to listen for IPC requests and handle them.

When the supervisor starts, it creates an `IpcServer` instance that listens for connections on `~/.local/state/pitchfork/ipc/main.sock`. Whenever a new client connects, the supervisor accepts the connection and creates dedicated read/write channels for it.

The supervisor uses an async task called `conn_watch` to continuously listen for IPC requests. This task runs in an infinite loop and never exits (function signature is `-> !`). Each time it receives a request, it:

1. Reads a message from the IPC server (including request content and response channel)
2. Immediately spawns a new tokio task to handle this request
3. Continues looping, waiting for the next request

The beauty of this design is that each request is handled in an independent task, so multiple CLI commands can execute concurrently without blocking each other. For example, you can run `pitchfork start app1` and `pitchfork start app2` simultaneously, and both commands will be processed in parallel.

The request handling task calls the `handle_ipc()` method, which dispatches to different handler functions based on request type. For `IpcRequest::Run` requests, it calls `self.run(opts).await`.

If an error occurs during processing, the supervisor converts the error to an `IpcResponse::Error` message and returns it to the client, rather than crashing the entire supervisor. This error handling strategy ensures supervisor stability.

### Phase 4: Running the Daemon

When the supervisor's `run()` method is called, the actual process startup flow begins.

First, the supervisor checks if this daemon is already running. It queries in-memory state information to see if there's a daemon with this ID and whether it has an active PID. If the daemon is already running and the user didn't specify the `--force` option, the supervisor directly returns a `DaemonAlreadyRunning` response to avoid duplicate starts. If `--force` was specified, the supervisor first stops the old process, then continues with starting a new one.

Next comes retry logic handling. If the configuration specifies `retry > 0` and `wait_ready` is true, the supervisor enters a retry loop. This loop executes at most `retry + 1` times (initial attempt + retry count).

In each attempt, the supervisor calls `run_once()` to start the daemon. If it returns `DaemonReady`, the start was successful and the result is immediately returned. If it returns `DaemonFailedWithCode`, the start failed, and the supervisor will:
1. Check if there are remaining retry attempts
2. If yes, calculate backoff time (power of 2 seconds: 1s, 2s, 4s, 8s...)
3. Wait for the backoff time, then enter the next loop iteration
4. If no attempts remain, return the failure response

This exponential backoff strategy is used in many distributed systems. Its benefits are:
- Gives failed services enough recovery time
- Avoids too-frequent retries causing system overload
- Increasing wait times can handle varying degrees of failure

If retry isn't needed (retry = 0 or wait_ready = false), the supervisor directly calls `run_once()` and returns the result.

### Phase 5: Spawning the Process

The `run_once()` method is where the process is actually created - this is the most complex part of the entire flow.

First, if `wait_ready` is true, the supervisor creates a oneshot channel. This channel consists of two parts: a sender (tx) and receiver (rx). The sender is passed to the monitoring task to notify whether the daemon is ready; the receiver remains in the current function to wait for the ready notification.

Then, the supervisor prepares the shell command to execute. There's an important trick here: adding the `exec` keyword before the command. `exec` is a shell built-in command that replaces the current shell process with a new program, rather than creating a subprocess. The benefits of this are:
- Reduces process hierarchy - the daemon's PID is the PID we record
- Avoids the shell process consuming extra resources
- Signal delivery is more direct - no need to handle intermediate shell processes when stopping

Next, the supervisor creates the log file. Each daemon has its own log directory at `~/.local/state/pitchfork/logs/<daemon-id>/`. If the directory doesn't exist, it's automatically created. The log file is named `<daemon-id>.log`.

Now the process can actually be started. The supervisor uses `tokio::process::Command` to start the process, an async process management tool provided by tokio. The startup configuration includes:
- stdin redirected to `/dev/null` (daemons typically don't need input)
- stdout and stderr both set to piped, so the supervisor can capture output
- Working directory set to the configured directory
- PATH environment variable set to the user's original PATH (not pitchfork's own)

After the process starts, the supervisor obtains its PID and immediately updates the state file. This step is important because even if the supervisor crashes, the state file will have a record of this daemon.

Then, the supervisor starts an independent monitoring task responsible for:
- Continuously reading the process's stdout and stderr
- Writing output to the log file
- Detecting readiness signals
- Monitoring process exit

Finally, if `wait_ready` is true, the current function waits on the channel's receiver. This wait can have three possible outcomes:
1. Receives `Ok(())`: daemon is ready, return `DaemonReady`
2. Receives `Err(exit_code)`: daemon failed to start, return `DaemonFailedWithCode`
3. Channel closes: exceptional situation, return `DaemonStart`

If `wait_ready` is false, the function immediately returns `DaemonStart` without waiting for ready notification.

### Phase 6: Monitoring the Process

The monitoring task is an independent tokio task whose lifecycle matches the monitored process.

After the task starts, it first obtains the process's stdout and stderr and wraps them as async line readers. It also opens the log file in append mode for writing.

Then, the task initializes readiness detection related state:
- `ready_notified`: marks whether a ready notification has been sent
- `ready_pattern`: if an output pattern is configured, compiles it into a regex
- `delay_timer`: if a delay time is configured, creates a timer

To simultaneously monitor multiple event sources, the task uses tokio's `select!` macro. This macro allows the task to wait for multiple async operations simultaneously, processing whichever completes first. In this scenario, the task needs to monitor:
1. New line of output on stdout
2. New line of output on stderr
3. Process exit
4. Delay timer trigger

When output is received from stdout or stderr, the task will:
1. Add a timestamp and daemon ID to this line of output
2. Write the formatted content to the log file and immediately flush (ensuring no loss)
3. If no ready notification has been sent yet, check if this line matches the ready_pattern
4. If it matches, mark ready_notified as true and send `Ok(())` through the channel

When the process exits, the task first checks if a ready notification has already been sent. If not, it means the process exited before becoming ready (startup failure), and the task extracts the exit code and sends `Err(exit_code)` through the channel. Then the task breaks out of the loop to prepare for cleanup.

When the delay timer triggers, if no output pattern is configured (ready_pattern is None), the task considers the daemon ready and sends the `Ok(())` notification. This implements time-based readiness detection.

After the select loop ends, the task needs to obtain the process's final exit status. There's a detail here: if the select loop ended because the process exited, the exit status is already saved in the `exit_status` variable; otherwise (e.g., stdout/stderr both closed), the task needs to wait for the process to exit.

Finally, the task updates the daemon's state. But before updating, it checks if the current daemon record still has this PID. This check is necessary because during monitoring, the user might have manually stopped this daemon and started a new one. If the PID doesn't match, this monitoring task is outdated and shouldn't update the state.

If the state update is necessary, the task judges based on exit code:
- Exit code 0 (success): set status to Stopped, mark last_exit_success as true
- Exit code non-0 (failure): set status to Errored, mark last_exit_success as false, preserve exit code

The state file is immediately written to disk, ensuring information persistence.


### Phase 7: State Persistence

The state file is the core mechanism for pitchfork's state persistence, located at `~/.local/state/pitchfork/state.toml`.

This file is stored in TOML format and contains three main sections:
1. `daemons`: a map recording all daemon state information
2. `disabled`: a set recording disabled daemon IDs
3. `shell_dirs`: a map recording shell PID to working directory mappings

Each daemon's state information includes:
- ID, PID, status (Running/Stopped/Errored)
- Working directory, whether autostop
- Cron scheduling related configuration
- Retry count and remaining attempts
- Readiness detection configuration
- Whether last exit was successful

State file reads and writes both use file locks (fslock) to ensure concurrent safety. When multiple processes attempt to read/write the state file simultaneously, the locking mechanism guarantees data consistency.

The `upsert_daemon()` method is responsible for updating or inserting daemon state. This method's logic is:
1. Acquire the state file lock
2. Check if there's already a record for this daemon
3. Create a new Daemon object; for unprovided fields, use values from existing record (if it exists)
4. Insert the new object into the map, replacing the old one
5. Serialize the entire state file to TOML format
6. Write to disk
7. Release the lock

This design has several advantages:
- Partial updates: caller can provide only the fields that need updating
- Atomicity: file lock guarantees update atomicity
- Recoverability: even if supervisor crashes, it can recover from the state file after restart
- Observability: users can directly `cat` the state file to view current state

An important use case for the state file is supervisor restart. When the supervisor restarts, it reads the state file and discovers that some daemon PIDs are still running. The supervisor can choose to:
- Continue monitoring these processes (reconnect)
- Or clean up outdated records

The current implementation periodically cleans up non-existent PIDs in the interval watcher.


### Phase 8: Response Back to CLI

When the monitoring task detects that the daemon is ready (or failed), the entire response flow begins to backtrack.

First, the monitoring task sends a ready notification through the oneshot channel. The `run_once()` function waits on the channel's receiver, and upon receiving notification constructs the appropriate `IpcResponse`:
- If it receives `Ok(())`, constructs `IpcResponse::DaemonReady`
- If it receives `Err(exit_code)`, constructs `IpcResponse::DaemonFailedWithCode`

Then, `run_once()` returns this response to the `run()` function. If retry is enabled, `run()` decides whether to continue retrying based on the response type. Ultimately, `run()` returns the final response to `handle_ipc()`.

`handle_ipc()` passes the response to the response channel in the `conn_watch()` task. This channel was created when accepting the IPC connection and connects to another independent sending task.

The sending task is responsible for serializing the `IpcResponse` to MessagePack format, adding the null byte delimiter, then writing to the Unix socket. This process is asynchronous and doesn't block the supervisor's other operations.

On the CLI side, the `IpcClient`'s `request()` method continuously calls `read()` in a loop, waiting for the response to arrive. The `read()` method reads data from the socket until encountering a null byte, then deserializes it into an `IpcResponse`.

After the CLI receives the response, the `run()` method processes it based on response type:
- If it's `DaemonReady`, records successfully started daemon ID
- If it's `DaemonFailedWithCode`, records failure and exit code
- If it's `DaemonAlreadyRunning`, prints a warning message

Finally, the CLI's `start` command prints appropriate messages based on the result and sets the exit code. If any daemon failed to start, the command exits with a non-zero exit code, which is useful in scripts.

The entire response chain is:
```
Monitoring task → oneshot channel → run_once() → run() → handle_ipc() 
  → response channel → sending task → Unix socket → IpcClient → CLI
```

## Background Watchers

Besides handling user requests, the supervisor also runs several background monitoring tasks that periodically check system state and perform automated operations.

### 1. Interval Watcher (10 second cycle)

The interval watcher is the supervisor's "heartbeat," executing a `refresh()` operation every 10 seconds.

The first task of `refresh()` is to refresh the process list. The supervisor uses the `sysinfo` crate to obtain information about all running processes in the system. This operation is relatively time-consuming, so it's not performed too frequently. After refreshing, the supervisor knows which PIDs are still running and which have died.

Next, the watcher checks shell PIDs. Pitchfork's shell integration feature records which shells are running in which directories. When a user executes `pitchfork start myapp` in a project directory, it records the current shell's PID. The watcher checks if these recorded shell PIDs are still running, and if it discovers a shell has exited, it will:
1. Remove this PID from `shell_dirs`
2. Check if there are other shells still running in this directory
3. If not, trigger the `leave_dir()` flow

`leave_dir()` is the implementation of the autostop feature. It looks for all daemons running in this directory (or subdirectories) that are marked with `autostop: true`. For each such daemon, it checks if there are other shells in its directory. If not, it stops the daemon.

This mechanism implements intelligent lifecycle management: when you start a development server in a project directory and then close the terminal, the server will automatically stop and won't continue consuming resources.

The final task of `refresh()` is to check daemons that need retrying. It iterates through all daemons with status Errored, checking:
- Whether there are remaining retry attempts (`retry_count < retry`)
- Whether it's currently not running (`pid.is_none()`)

For daemons meeting these conditions, the watcher automatically triggers a restart. This retry is "passive," different from the "active" retry in the `run()` method. Active retry is immediate retry when the user executes the start command; passive retry periodically checks and retries in the background.

### 2. Cron Watcher (60 second cycle)

The cron watcher implements cron-like scheduling functionality, but more flexible.

The watcher executes once per minute, iterating through all daemons configured with `cron_schedule`. For each daemon, it will:
1. Parse the cron expression (e.g., `"0 2 * * *"` means every day at 2 AM)
2. Calculate the next execution time
3. Check if the next execution time is within the next 60 seconds

If it should trigger, the watcher decides how to handle it based on the `cron_retrigger` configuration:
- `Finish`: only start if daemon is not running (default behavior)
- `Always`: force restart, even if currently running
- `Success`: only start if previous exit was successful
- `Fail`: only start if previous exit was failed

These modes cover common scheduling needs. For example, a data sync task can be configured with `Finish` mode to ensure the previous sync completes before starting a new one; a health check can be configured with `Always` mode to periodically restart to verify service availability.

The watcher reads the daemon's run command from `pitchfork.toml`, constructs `RunOptions`, then calls `self.run()` to start the daemon. The entire process is identical to manually executing `pitchfork start`, just triggered on a schedule.



