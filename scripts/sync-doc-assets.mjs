import { copyFile, mkdir } from 'node:fs/promises'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = resolve(root, 'docs/public/generated')

await mkdir(output, { recursive: true })
await Promise.all([
  copyFile(resolve(root, 'keysteer.default.toml'), resolve(output, 'keysteer.default.toml')),
  copyFile(resolve(root, 'assets/icons/keysteer-icon.png'), resolve(output, 'keysteer-icon.png')),
])
