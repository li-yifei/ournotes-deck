# 原生对照验证

歌曲统计、逐音符单局计算和编成搜索共用一个 Rust 计分模型。模型通过两类对照检查：在相同输入下与游戏客户端的原生实现逐帧比较，以及同一 Rust API 在本机与 WebAssembly 中的结果比较。本文说明比较方法和复现步骤。仓库不携带客户端、master 数据、谱面资源或原生捕获，复现时使用自己的客户端副本。

## 逐帧对照的方法

客户端的演出逻辑在离线环境中由 CPU 模拟器执行：谱面转换、Live 执行器、技能状态与触发、成员分配、效果池调度、技能条件、随机数和计分都运行客户端自身的代码。宿主环境提供文件系统、libc 和引擎服务的替身，替身只负责对象装配与展示文本，计分算术、技能条件、触发结果和判定结果都来自客户端代码。

原生自动输入接收受控的判定与时间参数，并记录实际输出的原始判定、转换后判定和顺序。Rust 模型在相同帧时刻消费这条原生判定流，独立地从谱面生成技能事件。两边逐帧记录状态后按字段契约比较：

- 整数字段必须完全相等；声明的 float32 字段按位模式比较。
- 不移动帧，不设误差界，不删除不一致的字段；契约要求的字段缺失时结果为 `incomplete`，不算通过。
- 逐帧比较还覆盖效果池身份、未发动的池、启停时间和排名前后两个取分阶段，因此能检查终分相等所不能单独保证的更新顺序。

比较数包含重复帧、空状态与池身份，是检查次数，不是独立样本数。

## 复现步骤

1. 准备客户端副本和对应的 master 数据，用 [`make_source_manifest.py`](../tools/native-validation/README.md) 的 `--resource ROLE=PATH` 记录输入身份（只记录哈希，不复制内容和本地路径）。
2. 在离线环境中运行客户端的演出逻辑，按比较器格式记录原生逐帧轨迹：顶层 `chartId` 与有序的 `frames`，每帧至少含 `frame`、`timeMs` 和契约声明的字段。
3. 用同一组判定驱动 `live::full::LiveModel`（`frame_timed` 逐帧推进），从其公开状态（`frame_score`、`score`、`current_life`、`current_combo`、`factor_state`、`gekisou_ranges` 等）导出同格式的轨迹。只比较分数、生命、连击与撃奏区间时，也可以直接用 `ournotes.replay/1` 请求的 `trace: true` 输出。
4. 写字段契约：整数字段、float32 位字段、数组长度和不比较的字段。[`ordinary-contract.json`](../tools/native-validation/ordinary-contract.json) 是普通技能矩阵的契约，其他技能配置要写明各自的效果池数量。
5. 运行比较器：

   ```text
   python tools/native-validation/compare_native_frames.py --native NATIVE.json --rust RUST.json --contract CONTRACT.json --output comparison.json
   ```

   退出码 0 表示声明字段全部相等，1 表示有差异，2 表示输入无效、不完整或缺失。输出保留第一处和全部差异、输入与契约的哈希。

## 单元级参考向量

判定、音符调度、辅助判定、各类 updater、判定窗口上限、成员顺序根、能力值与家具加成各有一组从客户端记录的参考向量。重放它们的测试在 `native-fixtures` feature 之后，测试代码即是各文件的 JSON 格式：

```text
OURNOTES_FIXTURES=/path/to/fixtures cargo test --release --features native-fixtures
```

缺少环境变量或文件时测试失败，不会跳过。不开 feature 时测试只使用合成数据。

## 本机与 WebAssembly

同一个 `ournotes.replay/1` 请求分别交给本机的 `ournotes_deck::replay::ReplaySession::run_json` 和 [`wasm/replay`](../wasm/replay/src/lib.rs) 的 `ReplaySession.run`。两边的分数、生命、逐帧最大连击、判定统计与撃奏区间必须相等，重复运行结果一致，含未知音符的请求两边都拒绝。普通 Live 与 SoloGekisou 都按这一方式比较。

## 共用模型的计算契约

逐音符重放以完成判定或完整 `RawResult` 为输入，明确帧时刻、原始判定顺序、技能配置、随机 seed 和排名策略。准度百分比不能唯一确定这些输入。页面的单局结果由共享 Rust 引擎运行产生；统计摘要分别输出名义期望区间与普通技能权重。4011 条件由完整模型处理，概率技能和条件交互按随机调用顺序运行；摘要使用各次判定独立的名义概率。

计分操作门控与生命、连击、回调处理分别保留；未执行条件池的执行与结束时间为 -1。Just 任务在原始判定前切换 Just 开关，结束时恢复客户端设置。音频/技能长度与计分表长度有独立来源：后者取最后音符位置的时间加 1,000 ms。适性分别计算最高判定与 Perfect 两端，并用完整名义期望校验区间预测。

单人排名会按区间时间查询计算器，多人排名使用控制器冻结的帧快照。`frame_score()` 表示排名前的显示缓存，`score()` 表示排名后的计算器值，两者是不同的观察阶段。固定名次统计使用 `new_gekisou_ranked` 的 Solo 取分定义；Network 模拟使用 `new_gekisou_external` 的帧快照，固定名次估计不模拟对手或服务器确认。

## 比较结果的含义

一次比较说明所声明的资源、输入、字段与阶段上的一致。以完成判定为输入时，比较验证的是下游的算术和状态转换；触控分类由单独的输入轨迹比较。契约中列为不比较的字段不参与结论。
