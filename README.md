# Tauri + React + Typescript

This template should help get you started developing with Tauri, React and Typescript in Vite.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)

# Init
## install
`````
PS C:\Users\yanan\code> mkdir fms-app
PS C:\Users\yanan\code> cd .\fms-app\
PS C:\Users\yanan\code\fms-app> cargo install create-tauri-app --locked

PS C:\Users\yanan\code> cargo create-tauri-app
✔ Project name · fms-app
✔ Identifier · com.wsva.fms-app
✔ Choose which language to use for your frontend · TypeScript / JavaScript - (pnpm, yarn, npm, deno, bun)
✔ Choose your package manager · pnpm
✔ Choose your UI template · React - (https://react.dev/)
✔ Choose your UI flavor · TypeScript

Template created! To get started run:
  cd fms-app
  pnpm install
  pnpm tauri android init

For Desktop development, run:
  pnpm tauri dev

For Android development, run:
  pnpm tauri android dev

PS C:\Users\yanan\code> cd .\fms-app\
PS C:\Users\yanan\code\fms-app> npx get-pnpm latest-12
`````

## pnpm install
need to open a new terminal to use the new PATH config.
`````
PS C:\Users\yanan\code\fms-app> pnpm install
`````

## start dev
`````
Start the development server:
pnpm tauri dev
`````

## change cargo source
there is not this file by default, so create it.

C:\Users\yanan\.cargo\config.toml
`````
[source.crates-io]
replace-with = 'rsproxy-sparse'

[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"
`````

