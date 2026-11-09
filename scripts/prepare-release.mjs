import { readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

/** Validate release inputs without putting secrets or machine-specific settings in the app source. */
export function createReleaseConfig({ repository, publicKey, versions, tag }) {
  if (typeof repository !== 'string' || !/^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})\/[A-Za-z0-9_.-]+$/.test(repository)) {
    throw new Error('GITHUB_REPOSITORY must be a GitHub owner/repo name.');
  }
  const [owner, repo] = repository.split('/');
  if (owner.endsWith('-') || repo === '.' || repo === '..' || repo.endsWith('.git')) {
    throw new Error('GITHUB_REPOSITORY must be a GitHub owner/repo name, without .git.');
  }

  const pubkey = typeof publicKey === 'string' ? publicKey.trim() : '';
  const keyError = 'TAURI_UPDATER_PUBLIC_KEY must contain the complete .pub file from `tauri signer generate`.';
  // Tauri's .pub file is base64 wrapping a minisign comment and a second base64 public key.
  const decoded = Buffer.from(pubkey, 'base64');
  if (!pubkey || decoded.toString('base64') !== pubkey) throw new Error(keyError);
  const lines = decoded.toString('utf8').trim().split(/\r?\n/);
  const key = Buffer.from(lines[1] ?? '', 'base64');
  if (
    lines.length !== 2 || !lines[0].startsWith('untrusted comment: ') ||
    key.toString('base64') !== lines[1] || key.length !== 42 ||
    key.subarray(0, 2).toString('ascii') !== 'Ed'
  ) {
    throw new Error(keyError);
  }

  const version = versions['src-tauri/tauri.conf.json'];
  // This workflow publishes one stable channel; prereleases must not become /releases/latest.
  if (typeof version !== 'string' || !/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version)) {
    throw new Error('Release version must be a stable SemVer version (for example, 0.1.1).');
  }
  for (const [name, value] of Object.entries(versions)) {
    if (value !== version) throw new Error(`Version in ${name} must match src-tauri/tauri.conf.json (${version}).`);
  }
  if (tag !== undefined && tag !== `v${version}`) {
    throw new Error(`Release tag must be v${version}.`);
  }

  return {
    bundle: { createUpdaterArtifacts: true },
    plugins: {
      updater: {
        pubkey,
        endpoints: [`https://github.com/${repository}/releases/latest/download/latest.json`],
      },
    },
  };
}

async function main() {
  const root = new URL('../', import.meta.url);
  const json = async (path) => JSON.parse(await readFile(new URL(path, root), 'utf8'));
  const [pkg, lock, tauri, cargo] = await Promise.all([
    json('package.json'), json('package-lock.json'), json('src-tauri/tauri.conf.json'),
    readFile(new URL('src-tauri/Cargo.toml', root), 'utf8'),
  ]);
  // This project's package version is a literal, not inherited from a Cargo workspace.
  const cargoPackage = cargo.split(/^\s*\[package\]\s*$/m)[1]?.split(/^\s*\[/m)[0] ?? '';
  const cargoVersion = cargoPackage.match(/^\s*version\s*=\s*["']([^"']+)["']\s*(?:#.*)?$/m)?.[1];
  if (process.env.GITHUB_ACTIONS === 'true') {
    if (process.env.GITHUB_REF_TYPE !== 'tag') throw new Error('Releases must run from a version tag.');
    if (process.env.RELEASE_REPOSITORY_PRIVATE !== 'false') {
      throw new Error('This release workflow requires a public GitHub repository for unauthenticated updates.');
    }
    if (!process.env.TAURI_SIGNING_PRIVATE_KEY?.trim()) {
      throw new Error('Set the TAURI_SIGNING_PRIVATE_KEY repository secret before releasing.');
    }
  }
  const config = createReleaseConfig({
    repository: process.env.GITHUB_REPOSITORY,
    publicKey: process.env.TAURI_UPDATER_PUBLIC_KEY,
    versions: {
      'package.json': pkg.version,
      'package-lock.json': lock.version,
      'package-lock.json packages[""]': lock.packages?.['']?.version,
      'src-tauri/tauri.conf.json': tauri.version,
      'src-tauri/Cargo.toml': cargoVersion,
    },
    tag: process.env.GITHUB_REF_TYPE === 'tag' ? process.env.GITHUB_REF_NAME : undefined,
  });
  await writeFile(new URL('src-tauri/tauri.updater.conf.json', root), `${JSON.stringify(config, null, 2)}\n`);
  console.log(`Prepared updater configuration for ${process.env.GITHUB_REPOSITORY} v${tauri.version}.`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
