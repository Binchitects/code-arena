---
name: explain-module
description: Explain how one module of this project works, from its entry points down, citing the code.
---
1. Find the module's entry points (its public functions, types or routes) with grep and glob.
2. Read them, then what they call, until the flow is clear.
3. Explain the module: its purpose, its main flow, its data, and its edges (errors, limits).
4. Cite every claim as path:line.
