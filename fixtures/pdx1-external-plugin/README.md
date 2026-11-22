# PDX-1 external plugin fixture

This project is intentionally outside the pnpm workspace. Its only Steward
dependency is the packaged `@steward/extension-api` tarball produced under
`target/sdk`, so installation exercises the same artifact external developers
receive.

From the repository root:

```powershell
pnpm sdk:pack
./scripts/test-pdx1-external.ps1
```
