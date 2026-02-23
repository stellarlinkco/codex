---
name: ralph-wiggum
description: 强制 Stop hook loop，避免 Codex 只做一步就停止；直到满足“完成承诺”才允许结束并输出总结。
hooks:
  Stop:
    - hooks:
        - type: command
          command: "python3 .claude/hooks/ralph-wiggum-stop-hook.py"
---

# Ralph Wiggum（循环防早停）

## 目的

- 解决“模型做完一步就停”的常见失败模式：在同一个 turn 内，Stop hook 会在模型尝试结束时进行拦截，并把你写在状态文件里的原始任务提示重新注入上下文，让模型继续迭代直到真正完成。
- 设计参考：Claude Code 的 ralph-wiggum 插件（Stop hook 驱动的自循环）。

## 使用时机

- 需要强制完成多步骤任务（例如 1/2/3/4/5 的实现步骤 + 最终总结）。
- 需要在自动化 loop 中设置“最大迭代次数”与“完成承诺”，避免无限循环。

## 快速开始（项目内文件）

1) 在项目根目录创建脚本（必须存在，否则 hook 会失败）：

- 路径：`.claude/hooks/ralph-wiggum-stop-hook.py`
- 内容：直接拷贝 `references/ralph-wiggum-stop-hook.py` 到上述路径。

2) 启动 loop：创建状态文件 `.claude/ralph-loop.local.md`

模板：

```md
---
iteration: 0
max_iterations: 20
completion_promise: "BUS-STOP-WEB-DONE"
---
请开发一个公交站台记录的 web 应用，按 1/2/3/4/5 步骤完成，并在最终输出中包含清晰的“总结”。
```

3) 结束 loop：仅当任务真的完成时，在最终回复中输出：

```txt
<promise>BUS-STOP-WEB-DONE</promise>
```

Stop hook 看到该标签后会自动删除状态文件并允许停止。

## 约束

- 这是“Stop hook 驱动的循环”，不是新工具；它只影响何时允许停止。
- 如果 `max_iterations > 0` 且达到上限，hook 会停止循环并删除状态文件，避免卡死。
