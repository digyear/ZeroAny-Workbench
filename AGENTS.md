# Clauge / ZeroAny-Workbench

## 项目身份
- 上游仓库：digyear/ZeroAny-Workbench
- 本地分支 feat/zeroany-workbench-branding 跟踪远端 feat/zeroany-pane-branding
- 发布：publish 走 v* tag

## 发布签名
- Tauri 发布密钥：~/Desktop/ZeroAny-Workbench-release-keys
- 签名轮换时可控密钥全部重生，并安全备份到桌面

## Worktree / 分支约定
- worktree 默认开启；关闭 worktree 须使用项目目录并警示冲突
- 分支命名 feat-<UUID8>/<base>-<UUID8>，可编辑
- APK 复用签名

## ZeroAny 视觉规范
- 保留羊毛毡/定格动画质感
- img2img 锁定轮廓比例、表情动作；字标另用 SVG
- 精简素材时忠实原构图与光感，只删次要元素
- 拒绝：自动减色描摹、过度抽象、重构原构图

<!-- ZEROANY-PURPOSE-START -->

You are working in a ZeroAny Workbench development session. Follow these rules strictly: Your process: 1. Understand the requested outcome before changing code 2. Read the relevant implementation and follow existing project conventions 3. Make small, focused changes without unrelated refactoring 4. Run the relevant checks and verify the actual behavior 5. If a requirement cannot be resolved from the project, ask instead of guessing

<!-- ZEROANY-PURPOSE-END -->
