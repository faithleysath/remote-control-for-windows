# rcwctl

这是 `rcwctl` 的 npm 元包。安装时会自动拉取当前平台对应的二进制包：

- `rcwctl-linux-x64`
- `rcwctl-linux-arm64`
- `rcwctl-darwin-x64`
- `rcwctl-darwin-arm64`
- `rcwctl-win32-x64`
- `rcwctl-windows-arm64`

```bash
npm install -g rcwctl
rcwctl --version
```

这些平台包直接把 `rcwctl` 二进制放进 npm tarball，所以 npm 镜像也能完整分发，不再依赖 GitHub Releases 的安装下载链路。
平台包本身也暴露 `rcwctl` 命令，但日常安装只需要元包。

## MCP

MCP 客户端可以直接用 `npx` 启动 stdio MCP 服务器，不需要自己下载二进制：

```json
{
  "mcpServers": {
    "rcw": {
      "command": "npx",
      "args": [
        "-y",
        "rcwctl",
        "--server",
        "ws://127.0.0.1:7800",
        "--token",
        "replace-with-control-token",
        "mcp"
      ]
    }
  }
}
```

MCP 进程内存保存远控 session 和后台传输任务状态，不读写普通 CLI 的本地 session 文件。agent 发送或接收文件使用 `upload` / `download` 路径型工具；文件主体走流式 WebSocket binary frame，不走 base64 参数。`upload` / `download` 默认等待 60 秒，未完成就返回 `task_id`，后续用 `transfer_status` 查询。
