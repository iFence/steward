# @steward/extension-api

Typed Steward host calls for NSH TSX plugins. The package is distributed as a
versioned tarball and exports compiled JavaScript plus TypeScript declarations.

```tsx
import { launchParams, toast } from "@steward/extension-api";

const params = launchParams();
await toast({ title: "Hello", message: `Plugin ${params.pluginId}` });
```

Install the tarball attached to a Steward GitHub release:

```powershell
pnpm add "https://github.com/iFence/steward/releases/latest/download/steward-extension-api-1.0.0.tgz"
```

See `docs/plugin-sdk-distribution.md` in the Steward repository for the
compatibility matrix, offline artifact workflow, and release command.
