# esp32-pokedex

运行在 **FoloToy AI Passport**（ESP32-C3）上的宝可梦图鉴固件。纯 Rust（`no_std` + [esp-hal](https://github.com/esp-rs/esp-hal)），
包含第一至第三世代全国图鉴 **386 只**宝可梦，官方简体中文译名，像素风界面。

## 特性

- **386 只全国图鉴**：官方中文名字/分类（妙蛙种子 · 种子宝可梦）
- **图鉴说明**：306 只使用官方中文文本（历代官方中文版游戏），80 只官方游戏未收录的采用 [52poke](https://wiki.52poke.com) 社区翻译
- **原版精灵图**：火红 64×64 正面图（4bpp 索引色 + 各自的 16 色调色板，出场动画为剪影闪现 + 上下回弹）
- **原版叫声**：386 只各自的 GBA 叫声，切到哪只就播哪只（长按连发浏览时不重复播，避免连珠炮）
- **随机 BGM**：开机时从 4 首火红城镇/道路曲目里随机抽一首循环播放（真新镇 / 1 号道路 / 宝可梦中心 / 玉虹市），叫声响起时自动压低伴奏
- **开机动画**：精灵球落下弹跳 → 晃动弹开 → 标题 → 扫描线切入图鉴（约 3 秒，任意键跳过），带弹跳/开球/翻页音效
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
| 音频 codec | ES8311，I2C 控制口 0x18：SDA=GPIO10，SCL=GPIO7 |
| 音频数据 | I2S0 主模式，16-bit 立体声：MCLK=GPIO6，BCLK=GPIO5，WS=GPIO3，DOUT=GPIO2 |
| 控制台 | 原生 USB Serial/JTAG（GPIO18/19） |

引脚事实来源为 [FoloToy/ai-passport](https://github.com/FoloToy/ai-passport) 的
`components/bsp/include/bsp_pins.h`，本仓库不重复定义。

## 操作

| 按键 | 卡片页 | 详情页 |
|---|---|---|
| 上 / 下 | 上一只 / 下一只（长按连发） | 同上，停留在详情页 |
| 确定（OK） | 进入详情页 | 返回卡片页 |

任何一次"上/下"单击都会播当前宝可梦的叫声并播放出场动画；长按连发时只播滚动音效，不重复播叫声。
开机动画进行中按任意键跳过。

## 构建

前置：Rust stable，安装 `riscv32imc-unknown-none-elf` 目标；[uv](https://docs.astral.sh/uv/)（用于运行 esptool）。

```bash
rustup target add riscv32imc-unknown-none-elf
```

1. **生成素材**（克隆后首次必须执行；二进制素材 `firmware/assets/` 不入库，`firmware/src/*_data.rs` 已随仓库提供）

   ```bash
   # 中文数据缓存（tools/zh_cache.json、tools/zh52poke_cache.json）已随仓库提供，
   # 无需联网；需要重新抓取时分别运行：
   #   python3 tools/fetch_zh.py        # PokeAPI 批量 CSV（官方中文）
   #   python3 tools/fetch_52poke.py    # 52poke（社区翻译，补官方缺口）

   python3 tools/gen_assets.py   # 精灵图（4bpp 索引色）+ 种族数据 + 字符集
   python3 tools/gen_font.py     # 点阵字库（需要 tools/fonts/NotoSansCJKsc-Regular.otf，见脚本说明）
   python3 tools/gen_audio.py    # 叫声 + BGM + 音效（约 4.4 MB）
   ```

   精灵图与音频都来自 [open-pokefirered](https://github.com/FoloToy/ai-passport) 的素材目录，
   路径写在脚本顶部的 `SRC`。`gen_assets.py` 会逐张校验 4bpp 打包无损，`gen_audio.py` 会
   检查总体积是否超预算。

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
core/        纯逻辑（主机可测）：按键分类/消抖、导航状态机、中文折行、属性表、音频混音器
firmware/    no_std 固件：
             st7789.rs / ui.rs     显示驱动与图鉴界面（支持局部刷新）
             audio.rs / boot.rs    ES8311 + I2S 流式 DMA 音频、开机动画
             memory.x              flash 窗口尺寸覆盖（理由见文件内注释）
tools/       素材流水线：数据抓取、精灵图/字库/音频打包、分区表生成
```

## 音频架构

三条声道汇成一个 16-bit 立体声流：**叫声**（切页触发）、**音乐**（开机随机 BGM，或开机 jingle）、
**音效**（翻页、开球、弹跳）。叫声响起时音乐在约 140 ms 内降到底、结束后升回，实测由
`core/src/audio.rs` 的单测覆盖。

- **采样率统一 11025 Hz**。叫声是 GBA 原生的 10512 Hz 8-bit 数据，直接按 11025 Hz 播放
  （快 4.9%，不到一个半音），因此 386 只叫声不需要重采样；BGM/音效在打包时重采样到这个速率。
  选 11025 Hz 是因为它和 8000/16000 Hz 一样能被 I2S 分频器**精确**合成（160 MHz / 2.8224 MHz
  = 56 + 304/441，误差 0 ppm），codec 侧也有对应的官方系数行。
- **各声道电平之和不超过满刻度**：叫声 52 / 音乐 60 / 音效 55（均为 0..255，三路同时到峰值
  为 167）。总音量靠 codec 的 `DAC_VOLUME`（-3 dB）补回来，而不是靠数字域加满——数字域一旦
  超过满刻度就是硬削波。
- **叫声响起时伴奏让开约 70 ms**（`duck_step`）：让开太慢的话，叫声起音会被伴奏盖住，听起来
  就像"叫声太小"，于是会误把叫声一路调小——真正该调的是让开速度。
- **DMA 是环形的**（`DmaTxStreamBuf`，8 个 1 KB 描述符 ≈ 186 ms）。主循环只负责把环重新填满，
  硬件永远接着往下读，所以换块不会断音。两个要点：起始只预填 3 个描述符（填满整圈会让写游标
  停在末尾，必须等整圈放完才能再写，等于每 186 ms 断一次）；推帧期间会在 SPI 分块之间回调
  `pump()`，否则一次 31 ms 的整屏推送就可能把 DMA 饿死。
- **BGM 素材是压缩流**：`open-pokefirered` 的 `bgm/*.wav` 里，标注 `codec: bfp` 的音轨
  装的是 6-bit 块浮点编码流而不是 PCM，直接当 PCM 读就是白噪声。`gen_audio.py` 调用导出处
  自带的解码器还原（整数实现，与 firered-app 的 Rust 解码器逐位一致），并用零穿越率卡一道
  校验，避免这类"看起来是 PCM 的噪声"再次混进来。
- **codec 起不来不会拖垮图鉴**：I2C 探测失败就只记一条日志，界面照常运行（`Player::tx` 为 `None`）。

### 为什么需要 `firmware/memory.x`

ESP32-C3 的 flash MMU 只有 **128 页 × 64 KB = 8 MB**，且指令窗口与数据窗口**共用**，
而 esp-hal 默认把两个窗口都写成 4 MB。精灵图（0.8 MB）+ 叫声（3.1 MB）+ BGM（1.2 MB）
必须一起住在这 8 MB 里，所以 `build.rs` 会用自己的 `memory.x` 覆盖 esp-hal 放在链接搜索
路径上的那一份（`linkall.x` 里的 `INCLUDE "memory.x"` 只会找到先出现的那份）。
当前镜像约 5.2 MiB，占 83/128 页，app 分区还剩 2.7 MiB 余量。

## 数据与素材来源

- 种族数据与精灵图：[open-pokefirered](https://github.com/FoloToy/ai-passport)（火红反编译数据）
- 叫声 / BGM / 音效：同上素材目录的 `cries/`、`bgm/`、`sfx/`（火红原声渲染）
- 官方中文名字/分类/图鉴文本：[PokeAPI](https://pokeapi.co) 批量数据（zh-Hans）
- 官方未收录的 80 只图鉴文本：[神奇宝贝百科（52poke）](https://wiki.52poke.com)
- 点阵字体：[Fusion Pixel Font](https://github.com/TakWolf/fusion-pixel-font)（OFL-1.1，协议见 `LICENSES/`）；个别缺字用 Noto Sans SC 兜底
- 硬件资料：[FoloToy AI Passport](https://github.com/FoloToy/ai-passport)

## 许可

代码以 [MIT 协议](LICENSE) 发布。

宝可梦及相关名称、精灵图等一切内容的版权归任天堂 / Creatures Inc. / GAME FREAK inc. 所有；
本项目仅供学习与个人使用，请勿用于商业用途。
