# esp32-pokedex

运行在 **FoloToy AI Passport**（ESP32-C3）上的宝可梦图鉴固件。纯 Rust（`no_std` + [esp-hal](https://github.com/esp-rs/esp-hal)），
包含第一至第三世代全国图鉴 **386 只**宝可梦，官方简体中文译名，像素风界面。

## 特性

- **386 只全国图鉴**：官方中文名字/分类（妙蛙种子 · 种子宝可梦）
- **图鉴说明**：306 只使用官方中文文本（历代官方中文版游戏），80 只官方游戏未收录的采用 [52poke](https://wiki.52poke.com) 社区翻译
- **原版精灵图**：火红 64×64 正面图（RGB565 + 1bpp 透明掩码，出场动画为剪影闪现 + 上下回弹）
- **双页图鉴 UI**（240×320 全屏绘制）：
  - 卡片页：精灵图、名字、分类、属性徽章（官方色板，浅底色自动深色文字）、身高体重
  - 详情页：种族值条形图 + 图鉴说明（中文按标点规则折行，约 22 字/行）
- **像素点阵字库**：Fusion Pixel 10px Monospaced（OFL-1.1），按用字子集嵌入，约 46 KB
- 上/下切换宝可梦（长按连发浏览），OK 在卡片页/详情页之间切换，翻页时保持当前页
- 2 分钟无操作自动关闭背光，任意按键唤醒

## 硬件与接线

| 外设 | 参数 |
|---|---|
| 主控 | ESP32-C3，8 MB Flash，无 PSRAM |
| 屏幕 | ST7789P3 240×320，4 线 SPI，SPI mode 0，出厂需反色（INVON） |
| 屏幕引脚 | SCLK=GPIO8，MOSI=GPIO9，CS=GPIO1，DC=GPIO20，无 RST（SWRESET）；背光=GPIO21 |
| 按键 | UP / DOWN / OK 共用一个 ADC 电阻梯：GPIO0（ADC1_CH0），电压窗口 UP<150mV / DOWN 150–447mV / OK 447–1900mV |
| 控制台 | 原生 USB Serial/JTAG（GPIO18/19） |

引脚事实来源为 [FoloToy/ai-passport](https://github.com/FoloToy/ai-passport) 的
`components/bsp/include/bsp_pins.h`，本仓库不重复定义。

## 操作

| 按键 | 卡片页 | 详情页 |
|---|---|---|
| 上 / 下 | 上一只 / 下一只（长按连发） | 同上，停留在详情页 |
| 确定（OK） | 进入详情页 | 返回卡片页 |

## 构建

前置：Rust stable，安装 `riscv32imc-unknown-none-elf` 目标；[uv](https://docs.astral.sh/uv/)（用于运行 esptool）。

```bash
rustup target add riscv32imc-unknown-none-elf
```

1. **生成素材**（克隆后首次必须执行；`firmware/assets/` 与 `firmware/src/*_data.rs` 不入库）

   ```bash
   # 中文数据缓存（tools/zh_cache.json、tools/zh52poke_cache.json）已随仓库提供，
   # 无需联网；需要重新抓取时分别运行：
   #   python3 tools/fetch_zh.py        # PokeAPI 批量 CSV（官方中文）
   #   python3 tools/fetch_52poke.py    # 52poke（社区翻译，补官方缺口）

   python3 tools/gen_assets.py   # 精灵图 + 种族数据 + 字符集（需要 open-pokefirered 数据，路径见脚本顶部 SRC）
   python3 tools/gen_font.py     # 点阵字库（需要 tools/fonts/NotoSansCJKsc-Regular.otf，见脚本说明）
   ```

2. **构建**

   ```bash
   cargo build -p pokedex-firmware --target riscv32imc-unknown-none-elf --release
   cargo test -p pokedex-core    # 主机侧逻辑单测
   ```

3. **烧录**

   ```bash
   uvx esptool --chip esp32c3 elf2image --flash_mode dio --flash_freq 80m --flash_size 8MB \
     target/riscv32imc-unknown-none-elf/release/pokedex-firmware -o build/pokedex.bin
   # 应用镜像写入 factory 分区（首次或改动分区表时，先写 tools/gen_partition_table.py 生成的分区表）
   python3 tools/gen_partition_table.py
   uvx esptool --chip esp32c3 -p /dev/cu.usbmodem2101 -b 460800 \
     write-flash 0x8000 build/partitions.bin 0x10000 build/pokedex.bin
   ```

   分区表把 factory 应用分区扩到剩余全部 Flash（0x7F0000），nvs/phy 保持与原厂一致。

## 项目结构

```
core/        纯逻辑（主机可测）：按键分类/消抖、导航状态机、中文折行、属性表
firmware/    no_std 固件：ST7789 驱动、点阵字库运行时、图鉴 UI、出场动画
tools/       素材流水线：数据抓取、精灵图/字库/分区表生成
```

## 数据与素材来源

- 种族数据与精灵图：[open-pokefirered](https://github.com/FoloToy/ai-passport)（火红反编译数据）
- 官方中文名字/分类/图鉴文本：[PokeAPI](https://pokeapi.co) 批量数据（zh-Hans）
- 官方未收录的 80 只图鉴文本：[神奇宝贝百科（52poke）](https://wiki.52poke.com)
- 点阵字体：[Fusion Pixel Font](https://github.com/TakWolf/fusion-pixel-font)（OFL-1.1，协议见 `LICENSES/`）；个别缺字用 Noto Sans SC 兜底
- 硬件资料：[FoloToy AI Passport](https://github.com/FoloToy/ai-passport)

## 许可

代码以 [MIT 协议](LICENSE) 发布。

宝可梦及相关名称、精灵图等一切内容的版权归任天堂 / Creatures Inc. / GAME FREAK inc. 所有；
本项目仅供学习与个人使用，请勿用于商业用途。
