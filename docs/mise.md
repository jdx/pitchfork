# Integrating pitchfork with mise

[mise](https://mise.jdx.dev) is a project for installing/managing dev tools, managing environment variables,
and running tasks. Unlike pitchfork, [mise tasks](https://mise.jdx.dev/tasks/) do not run in the background however
they offer a lot of functionality you won't find in pitchfork. It's encouraged to define relatively simple daemons
that just call `mise run` to launch the daemon as a mise task.

To do so, put the following into `pitchfork.toml`:

```toml
[daemons.docs]
run = "mise run docs:dev"
```

And in `mise.toml` you can define how `mise run docs:dev` gets setup and behaves:

```toml
[env]
NODE_ENV = "development"
[tools]
node = "20"
[tasks."docs:setup"]
run = "npm install"
[tasks."docs:dev"]
run = "node docs/index.js"
depends = ["docs:setup"]
```
