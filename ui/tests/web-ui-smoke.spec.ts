import { expect, test, type Page } from '@playwright/test'
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process'
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const repoRoot = path.resolve(__dirname, '../..')
const pitchforkBin = process.env.PITCHFORK_BIN
  ? path.resolve(process.env.PITCHFORK_BIN)
  : path.join(repoRoot, 'target/debug/pitchfork')

type WebSupervisor = {
  baseUrl: string
  cleanup: () => Promise<void>
}

type SupervisorOptions = {
  /** Extra project directories to create, keyed by name, with their pitchfork.toml. */
  projects?: Record<string, string>
  /** Global config contents, built from the created project directories. */
  globalConfig?: (dirs: Record<string, string>) => string
}

async function startWebSupervisor(options: SupervisorOptions = {}): Promise<WebSupervisor> {
  const root = await mkdtemp(path.join(tmpdir(), 'pitchfork-web-ui-'))
  const home = path.join(root, 'home')
  const project = path.join(root, 'project')
  await mkdir(path.join(home, '.config'), { recursive: true })
  await mkdir(path.join(home, '.local/state'), { recursive: true })
  await mkdir(project, { recursive: true })
  await writeFile(
    path.join(project, 'pitchfork.toml'),
    `[daemons.smoke]\nrun = "node -e 'setInterval(() => {}, 1000)'"\nready_delay = 0\n`,
  )

  const dirs: Record<string, string> = {}
  for (const [name, config] of Object.entries(options.projects ?? {})) {
    const dir = path.join(root, name)
    await mkdir(dir, { recursive: true })
    await writeFile(path.join(dir, 'pitchfork.toml'), config)
    dirs[name] = dir
  }
  if (options.globalConfig) {
    await mkdir(path.join(home, '.config/pitchfork'), { recursive: true })
    await writeFile(path.join(home, '.config/pitchfork/config.toml'), options.globalConfig(dirs))
  }

  const child = spawn(pitchforkBin, ['supervisor', 'run', '--web-port', '0'], {
    cwd: project,
    env: {
      ...process.env,
      HOME: home,
      XDG_CONFIG_HOME: path.join(home, '.config'),
      XDG_STATE_HOME: path.join(home, '.local/state'),
      PITCHFORK_LOG: 'debug',
      PITCHFORK_WEB_BIND_ADDRESS: '127.0.0.1',
      PITCHFORK_WEB_PATH: '',
      PITCHFORK_WATCH_INTERVAL: '100ms',
      PITCHFORK_WATCH_POLL_INTERVAL: '100ms',
    },
  })
  child.stdout.resume()

  const cleanup = async () => {
    await stopProcess(child)
    await rm(root, { recursive: true, force: true })
  }

  let stderr = ''
  let port: string
  try {
    port = await new Promise<string>((resolve, reject) => {
      let settled = false
      const finish = (callback: () => void) => {
        if (settled) return
        settled = true
        clearTimeout(timeout)
        callback()
      }
      const timeout = setTimeout(() => {
        finish(() => reject(new Error(`web supervisor did not start in time. stderr:\n${stderr}`)))
      }, 10_000)

      child.once('error', error => {
        finish(() => reject(new Error(`failed to spawn web supervisor: ${error.message}. stderr:\n${stderr}`)))
      })

      child.on('exit', (code, signal) => {
        finish(() => reject(new Error(`web supervisor exited early (${code ?? signal}). stderr:\n${stderr}`)))
      })

      child.stderr.on('data', (chunk: Buffer) => {
        stderr += chunk.toString()
        const match = stderr.match(/Web UI listening on http:\/\/127\.0\.0\.1:(\d+)/)
        if (match) {
          finish(() => resolve(match[1]))
        }
      })
    })
  } catch (error) {
    await cleanup()
    throw error
  }

  return {
    baseUrl: `http://127.0.0.1:${port}`,
    cleanup,
  }
}

async function stopProcess(child: ChildProcessWithoutNullStreams): Promise<void> {
  if (child.pid === undefined) return
  if (child.exitCode !== null || child.signalCode !== null) return

  await new Promise<void>((resolve) => {
    const forceKillTimeout = setTimeout(() => {
      child.kill('SIGKILL')
    }, 2_000)
    const giveUpTimeout = setTimeout(() => {
      resolve()
    }, 5_000)
    child.once('exit', () => {
      clearTimeout(forceKillTimeout)
      clearTimeout(giveUpTimeout)
      resolve()
    })
    child.kill('SIGTERM')
  })
}

function collectPageFailures(page: Page): string[] {
  const failures: string[] = []
  page.on('pageerror', error => failures.push(error.message))
  page.on('console', message => {
    if (message.type() === 'error') {
      failures.push(message.text())
    }
  })
  return failures
}

test('bundled web UI mounts, loads daemon data, and navigates routes', async ({ page }) => {
  const supervisor = await startWebSupervisor()
  const failures = collectPageFailures(page)

  try {
    await page.goto(supervisor.baseUrl)
    await expect(page.getByRole('heading', { name: 'Daemons', exact: true })).toBeVisible()
    await expect(page.getByText('smoke').first()).toBeVisible()

    await page.goto(`${supervisor.baseUrl}/proxies`)
    await expect(page.getByRole('heading', { name: 'Proxies', exact: true })).toBeVisible()
    await expect(page.getByText('No proxies registered')).toBeVisible()

    expect(failures).toEqual([])
  } finally {
    await supervisor.cleanup()
  }
})

test('project pages list projects, worktrees, and stack groups without auto-starting', async ({ page }) => {
  const supervisor = await startWebSupervisor({
    projects: {
      shop: [
        `[daemons.api]`,
        `run = "node -e 'setInterval(() => {}, 1000)'"`,
        `ready_delay = 0`,
        ``,
        `[daemons.worker]`,
        `run = "node -e 'setInterval(() => {}, 1000)'"`,
        `ready_delay = 0`,
        ``,
        `[groups.default]`,
        `daemons = ["api", "worker"]`,
        ``,
        `[groups.partial]`,
        `daemons = ["api", "ghost"]`,
        ``,
      ].join('\n'),
      blog: [
        `[daemons.site]`,
        `run = "node -e 'setInterval(() => {}, 1000)'"`,
        `ready_delay = 0`,
        ``,
      ].join('\n'),
    },
    globalConfig: dirs => [
      `[namespaces.shop]`,
      `dir = "${dirs.shop}"`,
      ``,
      `[namespaces.blog]`,
      `dir = "${dirs.blog}"`,
      ``,
    ].join('\n'),
  })
  const failures = collectPageFailures(page)

  try {
    await page.goto(`${supervisor.baseUrl}/projects`)
    await expect(page.getByRole('heading', { name: 'Projects', exact: true })).toBeVisible()
    await expect(page.getByRole('link', { name: 'blog', exact: true })).toBeVisible()

    await page.getByRole('link', { name: 'shop', exact: true }).click()
    await expect(page.getByRole('heading', { name: 'shop', exact: true })).toBeVisible()
    await expect(page.getByRole('link', { name: 'default', exact: true })).toBeVisible()
    await expect(page.getByRole('heading', { name: /Stack/ })).toBeVisible()

    // Opening a stack page starts nothing: the daemons are still available.
    const stackUrl = `${supervisor.baseUrl}/projects/shop/default`
    await page.goto(stackUrl)
    await expect(page.getByRole('heading', { name: 'default', level: 1 })).toBeVisible()
    await expect(page.getByRole('heading', { name: 'default', level: 3 })).toBeVisible()
    // Scoped to the default group: other groups show their own counts.
    const stack = page.locator('section').filter({
      has: page.getByRole('heading', { name: 'default', level: 3 }),
    })
    await expect(stack.getByText('0/2 running')).toBeVisible()

    // Starting is a click.
    await page.getByRole('button', { name: 'Start stack', exact: true }).click()
    await expect(stack.getByText('2/2 running')).toBeVisible()

    // Starting an already-running stack is a no-op per member, not a failure.
    // Wait for the first action's toast to clear so the assertions below can
    // only be satisfied by the second action's own result.
    await expect(page.getByText(/ started$/)).toHaveCount(0)
    await page.getByRole('button', { name: 'Start stack', exact: true }).click()
    await expect(page.getByText(/ started$/)).toBeVisible()
    await expect(page.getByText(/partially started|Start .* failed/)).toHaveCount(0)
    await expect(stack.getByText('2/2 running')).toBeVisible()

    await page.getByRole('button', { name: 'Stop stack', exact: true }).click()
    await expect(stack.getByText('0/2 running')).toBeVisible()

    // A group member no daemon matches is reported, not counted as success.
    const partial = page.locator('section').filter({
      has: page.getByRole('heading', { name: 'partial', level: 3 }),
    })
    await expect(partial.getByText(/Not defined in this worktree/)).toBeVisible()
    // The group's own action, not the member row's Start button.
    await partial.locator('.group-actions').getByRole('button', { name: 'Start', exact: true }).click()
    await expect(page.getByText(/partially started/)).toBeVisible()
    await expect(page.getByText(/ghost/).first()).toBeVisible()

    expect(failures).toEqual([])
  } finally {
    await supervisor.cleanup()
  }
})
