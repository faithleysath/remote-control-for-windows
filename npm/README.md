# @faithleysath/rcwctl

这是 `rcwctl` 的 npm 元包。安装时会自动拉取当前平台对应的二进制包：

- `@faithleysath/rcwctl-linux-x64`
- `@faithleysath/rcwctl-linux-arm64`
- `@faithleysath/rcwctl-darwin-x64`
- `@faithleysath/rcwctl-darwin-arm64`
- `@faithleysath/rcwctl-win32-x64`
- `@faithleysath/rcwctl-win32-arm64`

```bash
npm install -g @faithleysath/rcwctl
rcwctl --version
```

这些平台包直接把 `rcwctl` 二进制放进 npm tarball，所以 npm 镜像也能完整分发，不再依赖 GitHub Releases 的安装下载链路。
平台包本身也暴露 `rcwctl` 命令，但日常安装只需要元包。
