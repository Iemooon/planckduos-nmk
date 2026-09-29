# gazell-dongle — 单芯片 nRF52840：Gazell 2.4G 接收 + RMK + USB HID + Vial

一颗 nRF52840（测试硬件 PCA10059 dongle）同时做两件事：

- 用 **Nordic Gazell**（私有 2.4G 协议，host 角色）接收左右两个半键盘的矩阵，**严格单向**：发射端 → 本 dongle；
- 作为**一个 USB HID 键盘**（含 Vial 支持）接到电脑上。

原先的方案是「nRF51822 接收器 + STM32 跑 QMK，两者用 UART 连接」，本工程把它合并到一颗芯片上。

---

## 1. 数据是怎么流动的

```
Gazell host（中断里收包）
   ├─ 左半 pipe0：前缀1 + 基地址 0x01020304，只在自己的 {13,43,67} 三个信道上发
   └─ 右半 pipe1：前缀2 + 基地址 0x05060708，只在自己的 {27,53,77} 三个信道上发
        ↓ 每包 4 字节 = 4 行 × 6 列位图（bit0 = 该半最左列）
   RX 回调里立即取走（不等任务调度）
        ↓
   raw[row*2 + 半]  →  build_row(row) = 左 | (右 << 6)   ← 12 位逻辑行
        ↓
   RMK 自己的去抖器（DefaultDebouncer）+ 每键 KeyState
        ↓  KeyboardEvent::key(row, col, pressed)     ← 位置，不是键码
   RMK 的 Keyboard 处理器 → 键位表（由 Vial 拥有）→ 键码 → USB HID
```

**给 RMK 的是"矩阵坐标"，键位表把它变成键码。** 这与原版思路一致：原版接收端交给 QMK 一份合并好的矩阵，QMK 自己扫描、自己去抖。

---

## 2. 定制一把键盘要改哪些文件

### 99% 的情况：**只改 `board.toml`**（唯一真源）

`build.rs` 读它、校验它，然后生成 `memory.x`、`board_generated.rs`、`vial.json`、`config_generated.rs`。因此下面四件过去需要手工对齐的事——键位表维度、Gazell 行列打包、`vial.json` 的 matrix 段、USB 描述符——**不可能再互相漂移**。

| 你要做的事 | 改 `board.toml` 的哪一段 | 说明 |
|---|---|---|
| 换芯片 | `[chip] name` + 构建时 `--features <chip>` | 两者必须一致，否则 build.rs 直接报错 |
| 换板子/换 bootloader | `[memory]` 的 `flash_origin` / `flash_length` / `ram_origin` / `ram_length` | **`flash_origin` 是应用槽起点，取决于 bootloader，必须实测**（见 §4） |
| 改 USB 名称 / VID / PID | `[device]` | `vial_keyboard_id` 决定 Vial 把 `.vil` 布局绑定到哪块键盘，**同一把键盘不要改** |
| 换键盘形状 | `[matrix]` 的 `rows_per_half` / `cols_per_half` | `cols_per_half ≤ 8`（一行一个字节）、`rows_per_half ≤ 32`（Gazell 载荷上限）。半键盘数固定为 2 |
| 换发射端固件（协议参数变了） | `[gazell]` 的信道表 / 基地址 / 速率 / 时隙周期 | **必须与发射端逐项一致**，错一项就是"完全没有链路" |
| 换存储区位置 | `[flash] reserved_top` + `[storage]` | 必须夹在"应用区末尾"与"bootloader 起点"之间，build.rs 会校验 |
| 改键位 | **不用改代码**：在 Vial 里改；只有想改"出厂默认布局"时才动 `src/keymap.rs` | `src/keymap.rs` 的数组维度由 `board.toml` 推导，写错就是编译错误 |
| 加层 | `src/keymap.rs` 的 `NUM_LAYER` | 层数是该数组的编译期属性，也是 RMK 报告给 Vial 的层数。现在是 **8**；新层默认整层 `Transparent`（不会改变任何行为，只提供容量） |
| 加 combo / tap dance 上限 | **`keyboard.toml`** 的 `[rmk]`：`combo_max_num`、`morse_max_num` | 现在是 **32 / 32**。RMK 的 "tap dance" 就是它的 **morse**（同一套机制也做 tap-hold 与 home row mods）。这些是编译期容量，由 `rmk-types` 的构建脚本从 `KEYBOARD_TOML_PATH`（在 `.cargo/config.toml` 里指向本文件）读入；**没有这个文件时 RMK 静默用默认的 8 / 8** |

### 只有真正换板子才需要碰的代码

`src/` 里**没有任何芯片相关代码**（只用通用的 USBD / CLOCK / NVMC / POWER 与 RMK 的通用 API）。换芯片/换板子时**唯一**可能要动的是 `boards/` 下的板型 profile（复制 `board.toml` 改成新板子的值）。

> **构建期会拦住的错误**：芯片名与 Cargo feature 不一致、FLASH/RAM 超出该芯片真实容量、存储区压到应用代码上、存储区伸进 bootloader、矩阵维度越界、信道数 > 16。

---

## 3. 构建与烧录

**三个板型一次出全，并逐产物校验：跑 `tools\build-variants.cmd`。** 日常就用这一条。

手工构建某一个板型（纯 cargo，无需脚本）：

```powershell
# 在仓库根目录下
cargo build --release
cargo objcopy --release -- -O ihex gazell-dongle.hex
```

- `cargo objcopy` 需要一次性安装：`cargo install cargo-binutils` 与 `rustup component add llvm-tools`
- 产物 `gazell-dongle.hex` **自带绝对地址**（应用起点由 `board.toml` 的 `flash_origin` 经生成的 `memory.x` 固定）

**烧录：用 nRF Connect 的 Programmer**，Add file → 选 hex → Write。

> ⚠️ **绝对不要用整片擦除（Erase all）**：Nordic 的 USB DFU bootloader 在 `0xE0000`，擦掉后 USB 就再也烧不进去了（只能靠 SWD）。只写 hex 覆盖到的区域是安全的。

**验证设备**：插上后应出现 `VID_1313 / PID_4122` 的键盘，且 Vial 能识别；产品名可在设备管理器核对。

```powershell
Get-PnpDevice -PresentOnly | Where-Object InstanceId -match 'VID_1313' |
  ForEach-Object { (Get-PnpDeviceProperty -InstanceId $_.InstanceId `
      -KeyName DEVPKEY_Device_BusReportedDeviceDesc -ErrorAction SilentlyContinue).Data }
```

---

### 在线编译（GitHub Actions）

`.github/workflows/build.yml` 在 push / PR / 手动触发时跑两个 job：

* **`firmware`**：三个板型全部构建 + `tools/verify_variant.py` 产物级校验，成品作为 artifact 上传（仓库里不留二进制，见 `.gitignore`）。
* **`vendored-archive`**：纯 Python 解析仓库内那枚 Gazell 库，报告每成员的符号契约与出处指纹（成员集、内嵌源码路径、GCC 生产者串），并确认 `license.txt` 与二进制同行。

CI 不需要任何私有资产：`rmk` 从官方仓库按 `rev` 取，Gazell 库在 `vendor/gzll/`，工具链由 `rust-toolchain.toml` 钉为 **1.98.1**（本机实测通过构建的版本，且产物里嵌的 rustc commit `48a229cea` 与它吻合）。唯一需要从 apt 装的是 `gcc-arm-none-eabi`——因为 `adafruit_bl` 会拉进 rmk 的 BLE/crypto 路径，`p256-cortex-m4-sys` 在构建时要调 C 编译器；本机等价物由 `tools\build-variants.cmd` 把 QMK_MSYS 加进 PATH 解决。

> **已知且刻意接受的差异：CI 产物与本地产物不逐字节相同。** rustc 会把依赖源码的**绝对路径**嵌进 panic / assert 的位置字符串里，本地是 `C:\Users\lemon\.cargo\...`，runner 上是 `/home/runner/.cargo/...`。逻辑与地址布局一致，字节不一致。要做到位级一致，需要两侧统一加 `-C remap-path-prefix`——那会改变产物里的调试串，所以没有顺手加。**因此"能不能刷"看校验脚本的判据（起点、区间、family、四份互不相同），不要拿哈希当 CI 与本地的一致性证明。**

## 4. 换成别的板子



三个板型都能直接编译。**板型只描述"bootloader 和芯片占哪儿"，不是第二份 `board.toml`**：`build.rs` 从 `board.toml` 起步，再用 `BOARD_OVERRIDE` 指定的文件做深度合并（`merge_toml`）——覆盖文件里写了的键替换，没写的键继续从 `board.toml` 来。所以 `[matrix]`、`[gazell]`、`[device]` 和 Vial keyboard id 对三个板型是**同一份**，不会各抄一遍再各自漂移。

| 板子 | 覆盖文件（`BOARD_OVERRIDE`） | 产物 | 进 bootloader |
|---|---|---|---|
| PCA10059 dongle (52840) | 无（`board.toml` 本身即是） | `gazell-dongle-52840-dongle.hex` | 不用（SWD / DFU 烧录） |
| nice!nano (52840) | `boards\nice-nano-52840.toml` | `gazell-dongle-nicenano-52840.uf2`（+ 同名 .hex） | 双击 RST / 软件键 |
| blue macro (52833) | `boards\blue-macro-52833.toml` | `gazell-dongle-blue-macro-52833.uf2`（+ 同名 .hex） | 双击 RST / 软件键 |

52833 的板子必须加 `--no-default-features --features nrf52833`，而且要和覆盖文件里的 `[chip] name` 对上（`build.rs` 会校验）。

> 覆盖文件**不能**当 `BOARD_TOML` 用。它们不含 `[matrix]`/`[gazell]`，被当成全量配置读会在 `build.rs` 里直接 panic——这是刻意的：宁可构建失败，也不要静默产出一份布局错误的固件。

> **"blue macro" 是哪块板**：52833 上那块 UF2 板，bootloader 与外形都是 nice!nano 那一系（应用槽 `0x27000`、拖 `.uf2` 烧录），**在 keypoint-nmk 的 receiver 里同一块板的文件名叫 `nrf52833-nicenano`**——两个工程指的是同一块硬件，只是叫法不同。这里按使用者的叫法命名，避免"看文件名猜不到是哪块板"。
>
> 曾经还有一个 `52833-dk` 板型（PCA10100 开发板：自带 J-Link、**没有 bootloader**，应用从 `0x0` 开始、只能 SWD 烧），因为手上换成 blue macro 而移除；它的定义留在 git 历史（root commit `4d74c9c`），DK 若回来可以直接捞。"无 bootloader ⇒ 起点 `0x0`"这条规律本身仍记在 §4 末尾的 `flash_origin` 表里。

### nice!nano 52840 / blue macro 52833

芯片和 dongle 同类，差别全在 **Adafruit UF2 bootloader** 占用的低地址：

| 项 | nice!nano 52840 | blue macro 52833 |
|---|---|---|
| **应用槽起点** | **`0x1000`（RMK 布局）** | `0x27000`（ZMK 布局） |
| `flash_length` | `636K`（到 storage 为止） | `260K`（到 storage 为止） |
| `ram_origin` / `ram_length` | `0x20000008` / `255K` | `0x20000000` / `128K` |
| `reserved_top` | `0xF4000` | `0x74000`（**推算值，未实测**） |
| `start_addr` / `num_sectors` | `0xA0000` / 32 | `0x68000` / 12 |
| 烧录 | hex → uf2 拖拽 | 同左（family id 不同） |

```powershell
# uf2conv.py 来自 qmk_firmware / vial-qmk 的 util/ 目录，任选一份即可
$UF2CONV = '<path-to>\vial-qmk\util\uf2conv.py'

# nice!nano 52840
$env:BOARD_OVERRIDE = 'boards\nice-nano-52840.toml'
cargo build --release
cargo objcopy --release -- -O ihex gazell-dongle-nicenano-52840.hex
python $UF2CONV gazell-dongle-nicenano-52840.hex -c -f 0xADA52840 -o gazell-dongle-nicenano-52840.uf2

# blue macro 52833（只有 feature 和 family id 不同）
$env:BOARD_OVERRIDE = 'boards\blue-macro-52833.toml'
cargo build --release --no-default-features --features nrf52833
cargo objcopy --release --no-default-features --features nrf52833 -- -O ihex gazell-dongle-blue-macro-52833.hex
python $UF2CONV gazell-dongle-blue-macro-52833.hex -c -f 0x621E937A -o gazell-dongle-blue-macro-52833.uf2
```

日常不必手敲这些：`tools\build-variants.cmd` 一次做完三块板并校验。

要拖进 UF2 盘的是 `.uf2`。同名的 `.hex` **故意保留**：`tools\verify_variant.py` 要拿它和 uf2 对起点、对覆盖区间——一次转换把地址挪跑偏，产出的东西照样能"刷成功"然后不启动。

**family id 必须对**：它写在 UF2 头里，错了 bootloader 直接拒收。`0xADA52840` = NRF52840，`0x621E937A` = NRF52833（取自 `vial-qmk\util\uf2families.json`）。

### 最重要的一条：**应用槽在哪由 bootloader 决定，而且 RMK 和 ZMK 不一样**

官方 ZMK 的 nice_nano 把 SoftDevice 区（`0x0..0x26000`）留空、应用放 `0x26000`；**RMK 反过来，它回收这段 SoftDevice 区**，nRF52840 + Adafruit bootloader 的应用槽就是 **`0x1000`**：

```text
rmk/examples/use_config/nrf52840_ble/memory.x
  /* These values correspond to the nRF52840 WITH Adafruit nRF52 bootloader */
  FLASH : ORIGIN = 0x00001000, LENGTH = 1020K
  RAM   : ORIGIN = 0x20000008, LENGTH = 255K
```

RMK 的 FAQ 也写明了：*"Some nRF boards (i.e. the `nice!nano`) ship a bootloader that comes with a SoftDevice. RMK reclaims the flash region the SoftDevice occupies, while ZMK reserves it. So if you flash ZMK after RMK, the firmware might not work."*

**症状与判据**：bootloader 只跳它自己的应用槽，**写到别的地址不会报错**——UF2 照收、照写，然后永不启动，板子看起来完全死掉。所以判据是板子 U 盘里 `INFO_UF2.TXT` 的 `SoftDevice:` 那一行：`not found` 说明它没有 SD 预留、槽在 `0x1000`（本工程的 52840 nicenano 就是这种，`UF2 Bootloader 0.6.0`）。**别把 52833 那块板的 0x27000 抄过来，也别把 ZMK 的 0x26000 抄过来。**

### 两个稳压器（REG0 / REG1）也按 RMK 的芯片默认值写死

RMK 的 `[chip.nrf52840]` 默认是 `dcdc_reg0 = true`、`dcdc_reg1 = true`、`dcdc_reg0_voltage = "3V3"`。这件事必须显式声明，因为**应用启动时稳压器的模式不是芯片复位默认值**：Adafruit UF2 bootloader 会在跳进应用前把 REG1 切成 DC/DC，而 DC/DC 需要板上电感，并非每个版本都有。本工程对应 `board.toml`：

```toml
[power]
reg0 = "dcdc"    # "keep" 不碰 / "ldo" 强制 LDO / "dcdc" 强制 DC/DC
reg1 = "dcdc"
```

（`UICR.REGOUT0` 即 REG0 输出电压**故意不写**：RMK 注明改它需要 bootloader ≥ 0.10.0，而板子出厂就是 3V3。）

验证方式不是靠猜，而是反汇编看寄存器：`POWER.DCDCEN` = `0x40000578`、`DCDCEN0` = `0x40000580`，编译器会用"基址 0x40000518 + 偏移"，所以看到的是

```asm
    str.w r8, [r5, #96]    @ 0x40000518 + 0x60 = 0x40000578  (REG1 DC/DC)
    str.w r8, [r5, #104]   @ 0x40000518 + 0x68 = 0x40000580  (REG0 DC/DC)
```

### 怎么进 bootloader

**硬件方式（永远可用）**：**双击 RST**。nice!nano 上跑的是 Adafruit UF2 bootloader，靠"连续两次复位"识别，之后出现 U 盘，把 uf2 拖进去。

**软件方式**：在 Vial 里把任意一个键设成 **keycode `0x7C00`（Bootloader）**，按下就复位进 bootloader（RMK 的 VIA 命令 `BootloaderJump` 效果相同）。它依赖 `Cargo.toml` 里 rmk 的 **`adafruit_bl`** feature，本工程已启用：

```toml
rmk = { path = "...", default-features = false, features = [
    "defmt", "storage", "vial", "watchdog", "adafruit_bl",
] }
```

**没有这个 feature 时它不会工作**——原因见 §7 第一条：按键会普通复位回固件，看起来就是"这个键没反应"。

**注意 Nordic 系 bootloader 不认这个魔术值**：`adafruit_bl` 写的是 `0x57`（Adafruit UF2 的约定），而 Nordic DFU（PCA10059 那个 `0xE0000` 的 bootloader）认的是 `0xB1`。所以 **PCA10059 继续用 SWD + Programmer 烧录**，别指望软件进 DFU。

### 每块新板子唯一"必须实测"的参数：`flash_origin`

它是**应用槽的第一个字节**，由 bootloader 决定，不是芯片决定：

| 板子 | bootloader | 应用槽起点 |
|---|---|---|
| PCA10059 dongle (52840) | Nordic USB DFU | `0x1000` ← 已实测 |
| nice!nano 52840（这块） | Adafruit UF2 **0.6.0**，`SoftDevice: not found` | **`0x1000`（RMK 布局）** ← 已实测 |
| blue macro 52833（那块） | Adafruit UF2，预留 SoftDevice 区 | `0x27000`（ZMK 布局）← 已实测可刷 |
| 裸片、无 bootloader | — | `0x0` |

**同名的"nice!nano"可以是两种布局**，取决于出厂 bootloader 预留了多大的 SoftDevice 区——所以这两块板**不能互相抄参数**。判断方法就是读 U 盘里的 `INFO_UF2.TXT`（见上一节）。

**怎么实测**：让板子进 bootloader，然后

```powershell
nrfutil device fw-info --serial-number <sn>
```

取其中的 `imageLocation.address`。**填错的现象是"烧录成功、永不启动"。**

### 芯片前提：必须有 USB

**nRF52832 / 52811 / 52810 一律不能用**（RMK 的相关功能带 `_no_usb` 后缀）。可用的只有 **nRF52840 / nRF52833 / nRF52820**。

---

## 5. 必须知道的三个约束

### ① Gazell 库的版本（本项目最关键的教训）

本工程链接 **nRF5 SDK 17.1.1** 的那份 Gazell 主机库，已随仓库 vendored（clone 下来不需要装 SDK 就能编）：

```
vendor/gzll/gzll_nrf52840_gcc.a
```

出处与许可见 `vendor/gzll/README.md`；可用环境变量 `GZLL_DIR` / `GZLL_LIB` 指向别的 SDK 构建（见 `build.rs`）。

**为什么要强调版本**：最初链接 SDK 12.3 的 `gzll_nrf52_gcc.a` 时，**主机对第二个 pipe 的服务只有第一个的一半**（实测：pipe 0 收到 150–299 包/秒，pipe 1 只有 50–149），表现为"**哪半边不在 pipe 0 上，哪半边就卡顿**"，且与硬件、发射端、半边键盘都无关（三方对调实验证实症状只跟 pipe 索引走）。当时把信道表、时隙数、功率、同步寿命、包池、取包时机、去抖、事件渠道、事件顺序全部排查并排除——**全部无效，因为那个库本身是恒定不变的**。换到 SDK 17.1.1 为 nRF52840 单独编译的产物后问题消失。

**换库前先用 `arm-none-eabi-nm` 核对符号契约**：它需要我们从外部提供 `memcpy`、`memset` 以及 4 个回调（`nrf_gzll_host_rx_data_ready`、`nrf_gzll_device_tx_success`、`nrf_gzll_device_tx_failed`、`nrf_gzll_disabled`），并自带 `TIMER2_IRQHandler`、`RADIO_IRQHandler`、`SWI0_EGU0_IRQHandler`。两个 SDK 版本恰好一致，所以是纯替换。

> 原键盘（nRF51822）用的是 **SDK 11.0.0** 的 `gzll_gcc.a`（路径里 `components/properitary_rf/` 那个拼写错误是 11.x 的特征），是 nRF51 产物，**不能在 nRF52840 上运行**（外设基地址与寄存器布局不同）。

### ② 两个半键盘的地址固定且必须不同

左半 `0x01020304`、右半 `0x05060708`。Gazell 的 `base_address_0` **只作用于 pipe 0**，而 pipe 1..7 共用一个基地址、靠前缀字节区分，所以：

- 左半**只能**挂在 pipe 0；
- "把两个半键盘都放到 pipe 1/2"在机制上不可能；
- 两个半键盘用同一个地址也不行：主机无法区分它们，两半的包会挤进同一个 pipe 互相覆盖。

### ③ 与单芯片方案唯一强制的差异：晶振控制

`nrf_gzll_set_xosc_ctl(MANUAL)`。Gazell 默认的 `AUTO` 会在时隙之间关掉 32 MHz 晶振，**USB 会跟着一起没**。所以本工程让 HFXO 常开（见 `src/main.rs` 的 `prepare_clocks`），并显式告诉 Gazell 采用手动控制。

---

## 6. 已知限制

- **看门狗没接。** RMK 的 `Nrf52Watchdog` 由 `any(feature = "_nrf_ble", feature = "dfu_nrf")` 门控；`adafruit_bl` 为了写 bootloader 魔术值依赖 `_nrf_ble`，所以它现在**已经可用了**，但本工程没有把它交给运行器，等于仍未启用。要接就是建一个 `Nrf52Watchdog` + `WatchdogRunner` 放进 `run_all!`。
- **BLE 不参与**：本方案是纯 Gazell + USB，所以 MPSL / SoftDeviceController 全部不用，RADIO 完全归 Gazell。（`adafruit_bl` 带进来的 BLE 代码只是被编译进镜像、从不初始化。）
- **发射端不可重刷**：本工程只实现"接收端"。协议参数（信道表、基地址、速率、时隙周期、载荷格式）由发射端固件决定，**不可更改**。
- **按键是快照语义**：发射端空闲 0.5 秒后自行断电，按键时冷启动并重新同步；按键期间每 5 ms 一个包。因此实时性取决于链路投递速率，主机侧不做任何"补齐"。

---

## 7. 关键实现注意事项（踩过的坑）

* **"软件进 bootloader 的键没反应" = rmk 少了 `adafruit_bl` feature**（本工程已加）。`rmk/src/boot.rs` 只在 `adafruit_bl` / `rp2040` / `zsa_voyager_bl` 三者之一启用时才写魔术值，否则只 `warn!("Please specify a bootloader to jump to!")` 一句再 `sys_reset()`——芯片复位后**又回到键盘固件**，所以现象就是"按了没反应"（复位其实发生了）。启用后写的是 **`0x57` → `POWER->GPREGRET`**，在 nRF52840/52833 上这个寄存器是 **`0x4000051C`**，也就是 Zephyr 里的 `gpregret1@51c`、ZMK 两个 `*_uf2_boot_mode.dtsi` 里 `&gpregret1` 指的那个——Adafruit UF2 bootloader 正是在这里等 `0x57`。代价是拉进 `_nrf_ble`（trouble-host / bt-hci），但 BLE 从不初始化，所以只花体积（52833 实测产物约 140 KB，对 260K 的应用槽毫无压力）。
* **52840 nice!nano 刷完"毫无反应"，先怀疑 REG1 的 DC/DC**：Adafruit UF2 bootloader 会在跳进应用前把 REG1 置为 DC/DC，而 DC/DC 依赖板上电感，并非所有 nice!nano 版本都有；芯片复位默认是 LDO，所以不碰 REG1 的固件会继承 DC/DC，在没有电感的板子上供电会随射频/USB 电流一起塌。官方 ZMK 为此按板写死 `&reg1`，本工程对应 `board.toml` 的 `[power] reg1`（说明见 §4）。注意**换错应用槽偏移也会有同样的"死板"现象**，两者要先分清：读一下板子的 `INFO_UF2.TXT`（双击 RST 后 U 盘里那个文件）确认 bootloader 型号/版本，必要时用官方 ZMK 在**同一块板**上做一次对照。
* **Gazell 的晶振控制要设 `MANUAL`**：否则它会在时隙之间关掉晶振，USB 跟着一起没。
* **取包必须"越早越好"，且不能门控在回调标志位上**：RX FIFO 只有 3 格、两个 pipe 共用 6 个包的总池（`NRF_GZLL_CONST_MAX_TOTAL_PACKETS`），而 Gazell 只对它存得下的包回 ACK。所以：在收包回调里**立刻**取空，并且**两个 pipe 一起取空**（否则通知被丢的那一侧会一直占着包池，把另一侧饿死）。原版接收端是主循环每 ~10 µs 轮询 FIFO，效果等价。
* **在回调里不要调用库的其他 API**：原版接收端的回调是空函数、只在主循环里轮询与取包。额外调用会引入难以排查的变量。
* **不要在 PowerShell 5.1 里用 `Get-Content -Raw | Set-Content` 处理带中文的文件**：它按 ANSI 读、按 UTF-8 写，会把中文变成乱码。改中文文件请用编辑器。
* **`Cargo.toml` 里的 `rmk` 是官方仓库的 git 依赖，钉死在某个 commit**（`rev = "f12257e8"`，RMK 0.9.0 未发布到 crates.io）。用 git 而不是本地路径，是"别人 clone 下来也能编、CI 也能编"的前提；钉 rev 而不是分支，是因为 rmk 的宏决定链接产物，跟住 `main` 会让一次构建无法追溯到具体源码，而且 runner 和本机会各自编出不同的固件。升级 rmk 应当是一次有意识的改动，不是某次 `cargo update` 的副作用。
* **Vial 的"解锁"要能走完，必须有 rmk 的 `host_lock` feature**（本工程已加）。`insecure: true` 只决定"状态永远是已解锁"，而 `GetUnlockStatus(0x0D)` / `UnlockStart(0x0E)` / `UnlockPoll(0x0F)` 的**应答**整段被 `#[cfg(feature = "host_lock")]` 门控：没有它，这三条命令只打印一句 `error!("Vial lock feature is not enabled")`、**一个字节都不回**。于是 Vial GUI 的解锁握手永远结束不了——点界面上的"进入 bootloader"就一直停在"请输入解锁键"对话框上，而固件里根本没有键位可给它。**键码 `0x7C00` 不经过这条路，所以键是好的、按钮是坏的。** RMK 的默认 feature 里本来就有 `host_lock`，本工程用 `default-features = false`，所以必须显式列出。
* **改键不需要解锁**：`VialConfig` 直接构造、`insecure: true`，`HostLock::is_unlocked()` 恒为真（`VialConfig::new()` 会把 `insecure` 写死为 `false`），改键位和矩阵测试都无需按任何组合键。`unlock_keys` 仍给了左半第一行前两个键 `(0,0)`、`(0,1)`——**不是要你去按**，而是让 GUI 拿到的解锁键列表有效（空表时 GUI 只能给出一个按不了的提示，键位字节还会是 `0xFF`）；键位是矩阵 `(row, col)`，12 列里 `0-5` 是左半、`6-11` 是右半。

---

## 8. 文件一览

| 文件 | 作用 |
|---|---|
| **`board.toml`** | **唯一真源**：芯片、内存布局、USB 标识、矩阵形状、Gazell 参数、存储区 |
| `build.rs` | 读 `board.toml`（再深度合并 `BOARD_OVERRIDE`）→ 校验 → 生成 `memory.x` / `board_generated.rs` / `vial.json` / `config_generated.rs`，并链接 Gazell 库 |
| `src/main.rs` | 时钟准备（HFXO 有界等待、停 LFCLK）+ 装配 RMK 与 UsbTransport |
| `src/gazell.rs` | 射频：FFI、4 个回调、3 个中断向量跳转、中断级取包、矩阵合并、RMK 去抖 |
| `src/board.rs` | 派生常量（行/列/掩码）+ 编译期不变量 |
| `src/keymap.rs` | 出厂默认键位（实际布局由 Vial 拥有并存在 flash 里） |
| `src/vial.rs` | 生成的 Vial 配置 |
| `boards/*.toml` | **板型覆盖文件**（只写 `[chip]`/`[memory]`/`[flash]`/`[storage]`/`[power]` 里与 bootloader 有关的那几个键），用 `BOARD_OVERRIDE` 选择，由 `build.rs` 的 `merge_toml` 深度合并到 `board.toml` 上 |
| `tools\build-variants.cmd` | 一键出三个板型全部产物（3×hex + 2×uf2），末尾自动跑校验 |
| `tools\verify_variant.py` | **按产物**校验四板型：起点/区间/是否压到 storage 或 bootloader、uf2 family、uf2 与 hex 是否同一镜像、该不同的镜像是否真的不同 |
| `tools\hex_span.py` | 读 Intel HEX 的地址区间（type 02 与 04 都认）；分区百分比交给上面那个脚本，这里不假设某一块板的大小 |
| `Cargo.toml` / `Cargo.lock` | 依赖与锁定版本 |
| **`keyboard.toml`** | RMK 的**编译期行为参数**（`debounce_time` 5、combo 32 / morse 32）。**不是板子描述**——板子的事只在 `board.toml`。由 `.cargo/config.toml` 的 `KEYBOARD_TOML_PATH` 指给 `rmk-types` |
| `.cargo/config.toml` | 目标三元组、flip-link、defmt 日志级别 |

---

## 9. 许可

源码采用 **Apache-2.0 与 MIT 双授权**（`LICENSE-APACHE` / `LICENSE-MIT`），与 RMK 本体的做法一致，任选其一即可。

**这不适用于 `vendor/gzll/`。** 那里是 Nordic Semiconductor 的闭源预编译库，依 `vendor/gzll/license.txt`（BSD-3 风格）的条款随发行版分发：版权条、条件列表与免责必须与二进制同行（所以 `license.txt` 就放在它旁边），且只能用于 Nordic 的集成电路。整个 2.4G 链路都建立在这一个文件上，出处、成员清单与"为什么是 17.1.1 而不是更老的版本"记在 `vendor/gzll/README.md`。
