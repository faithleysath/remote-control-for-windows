# @faithleysath/rcwctl

这个 npm 包安装 GitHub Release 中预编译的 `rcwctl` 控制端 CLI。

```bash
npm install -g @faithleysath/rcwctl
rcwctl --version
```

支持的预编译 npm 平台：

- Linux x86-64 glibc
- Linux arm64 glibc
- macOS x86-64
- macOS arm64
- Windows x86-64
- Windows arm64

包安装时会下载与 `package.json` 版本相同的 GitHub Release 产物，例如 `v0.1.0` 对应 npm 版本 `0.1.0`。
