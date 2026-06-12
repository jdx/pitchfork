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

async function startWebSupervisor(): Promise<WebSupervisor> {
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

  const child = spawn(pitchforkBin, ['supervisor', 'run', '--web-port', '0'], {
    cwd: project,
    env: {
      ...process.env,
      HOME: home,
      XDG_CONFIG_HOME: path.join(home, '.config'),
      XDG_STATE_HOME: path.join(home, '.local/state'),
      PITCHFORK_LOG: 'debug',
      PITCHFORK_WATCH_INTERVAL: '100ms',
      PITCHFORK_WATCH_POLL_INTERVAL: '100ms',
    },
  })
  child.stdout.resume()

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
    await stopProcess(child)
    await rm(root, { recursive: true, force: true })
    throw error
  }

  return {
    baseUrl: `http://127.0.0.1:${port}`,
    cleanup: async () => {
      await stopProcess(child)
      await rm(root, { recursive: true, force: true })
    },
  }
}

async function stopProcess(child: ChildProcessWithoutNullStreams): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return

  await new Promise<void>((resolve) => {
    const timeout = setTimeout(() => {
      child.kill('SIGKILL')
      resolve()
    }, 2_000)
    child.once('exit', () => {
      clearTimeout(timeout)
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
