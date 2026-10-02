# Releasing

For maintainers.

## Cutting a release

1. Bump the version in `Cargo.toml` (`[workspace.package]`), `package.json` and
   `src-tauri/tauri.conf.json`.
2. Regenerate the notices if dependencies changed: `python3 scripts/third-party-notices.py`.
3. Commit, then tag and push:

```bash
git tag vX.Y.Z && git push origin main vX.Y.Z
```

## What the pipeline does

The [Build workflow](../.github/workflows/build.yml) then:

1. Builds and tests the app, and checks that `THIRD_PARTY_NOTICES.md` is up to date.
2. **Waits for approval** in the `release` environment. Nothing is signed or published until a
   reviewer approves the run (Actions → the run → *Review deployments*).
3. Signs the executable.
4. Builds and signs the Inno Setup installer. It installs `LICENSE`, `THIRD_PARTY_NOTICES.md` and
   `THIRD_PARTY_LICENSES.txt` next to the executable.
5. Builds the portable zip, with the same license files.
6. Smoke-tests install, upgrade and uninstall on a clean runner.
7. Publishes the GitHub release with SHA-256 sums.

**Other builds.** Pushes and pull requests to `main` only build and test. *Run workflow* on the
Actions tab makes an unsigned test installer that is never published.

## Code signing

Releases are signed with Azure Trusted Signing through GitHub's OIDC federation, so no certificate
or password is stored in the repository or its settings. Signing switches on once the repository
has:

- **Secrets:** `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_SUBSCRIPTION_ID`
- **Variables:** `SIGNING_ENDPOINT`, `SIGNING_ACCOUNT`, `SIGNING_PROFILE`

Until then, releases are built unsigned.

**Azure side.** The app registration needs two things:

- a federated credential whose subject matches this repository's `release` environment
- the *Trusted Signing Certificate Profile Signer* role on the certificate profile

**Subject format.** The repository uses GitHub's immutable-ID OIDC subject format, not the older
`repo:owner/repo` one. Read the exact prefix with:

```bash
gh api repos/<owner>/<repo>/actions/oidc/customization/sub
```

Then append `:environment:release` to it.
