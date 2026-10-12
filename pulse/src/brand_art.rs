// Pulse 的图标：圆角方块上的点阵心跳线。
//
// 构图参考 Windows 11 任务管理器的图标（方块 + 起伏的折线），改成和应用里图表一样按点绘制：
// 每列最高的点是纯白（连起来就是心跳折线），折线下方的点半透明作为填充，没点亮的点只留很淡的底色。
// 本文件不依赖任何库：build.rs 用 `include!` 引入它来生成 exe 的 .ico 资源，程序本身作为模块使用
// （窗口图标、界面贴图），所以这里只放普通函数，不写 `use`、不写模块级文档注释。

/// 不同尺寸用不同密度的点阵，保证小图标的点仍有 2 像素以上：(列数, 各列点亮的点数)。
fn brand_pattern(size: u32) -> (usize, &'static [usize]) {
    if size <= 20 {
        (4, &[1, 4, 1, 2])
    } else if size <= 40 {
        (5, &[2, 1, 5, 2, 3])
    } else {
        (7, &[2, 3, 2, 7, 1, 4, 3])
    }
}

/// 背景渐变：左上 → 右下。
const BRAND_FROM: [f32; 3] = [0x3b as f32, 0x6e as f32, 0xf6 as f32];
const BRAND_TO: [f32; 3] = [0x14 as f32, 0xc8 as f32, 0xe6 as f32];

/// 渲染 `size`×`size` 的图标，返回非预乘的 RGBA。
pub fn brand_rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    let (cols, heights) = brand_pattern(size);
    let rows = cols;

    // 圆角方块，四周留出 Windows 图标惯用的边距。
    let pad = (s / 16.0).round().max(1.0);
    let side = s - 2.0 * pad;
    let radius = side * 0.24;

    // 点阵：点距取整数像素，网格在方块里居中。
    let grid_max = side * 0.7;
    let pitch = (grid_max / cols as f32).floor().max(2.0);
    let dot = (pitch * 0.7).round().clamp(1.0, pitch - 1.0);
    let grid = pitch * (cols as f32 - 1.0) + dot;
    let gx = (pad + (side - grid) / 2.0).round();
    let gy = (pad + (side - grid) / 2.0).round();
    let dot_radius = if size >= 48 { dot * 0.3 } else { 0.0 };

    let inside_rounded = |x: f32, y: f32, x0: f32, y0: f32, w: f32, r: f32| -> bool {
        if x < x0 || y < y0 || x > x0 + w || y > y0 + w {
            return false;
        }
        let cx = x.clamp(x0 + r, x0 + w - r);
        let cy = y.clamp(y0 + r, y0 + w - r);
        (x - cx).powi(2) + (y - cy).powi(2) <= r * r
    };

    // 每个子采样点的颜色（预乘），4×4 超采样抗锯齿。
    const SS: usize = 4;
    let mut out = vec![0u8; (size * size * 4) as usize];
    for py in 0..size {
        for px in 0..size {
            let mut acc = [0f32; 4];
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = px as f32 + (sx as f32 + 0.5) / SS as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / SS as f32;
                    if !inside_rounded(x, y, pad, pad, side, radius) {
                        continue;
                    }
                    let t = ((x - pad) + (y - pad)) / (2.0 * side);
                    let mut c = [
                        BRAND_FROM[0] + (BRAND_TO[0] - BRAND_FROM[0]) * t,
                        BRAND_FROM[1] + (BRAND_TO[1] - BRAND_FROM[1]) * t,
                        BRAND_FROM[2] + (BRAND_TO[2] - BRAND_FROM[2]) * t,
                    ];
                    // 落在哪个点上：按格子直接算，不用逐点比较。
                    let col = ((x - gx) / pitch).floor();
                    let row_from_top = ((y - gy) / pitch).floor();
                    if col >= 0.0 && row_from_top >= 0.0 {
                        let (col, row_from_top) = (col as usize, row_from_top as usize);
                        if col < cols && row_from_top < rows {
                            let dx = gx + col as f32 * pitch;
                            let dy = gy + row_from_top as f32 * pitch;
                            if inside_rounded(x, y, dx, dy, dot, dot_radius.max(0.01)) {
                                let row = rows - 1 - row_from_top; // 从底部数
                                let lit = heights[col];
                                let alpha = if row + 1 == lit {
                                    1.0
                                } else if row < lit {
                                    0.38 + 0.2 * row as f32 / lit as f32
                                } else {
                                    0.12
                                };
                                for ch in &mut c {
                                    *ch += (255.0 - *ch) * alpha;
                                }
                            }
                        }
                    }
                    acc[0] += c[0];
                    acc[1] += c[1];
                    acc[2] += c[2];
                    acc[3] += 1.0;
                }
            }
            let i = ((py * size + px) * 4) as usize;
            if acc[3] > 0.0 {
                out[i] = (acc[0] / acc[3]).round() as u8;
                out[i + 1] = (acc[1] / acc[3]).round() as u8;
                out[i + 2] = (acc[2] / acc[3]).round() as u8;
                out[i + 3] = (acc[3] / (SS * SS) as f32 * 255.0).round() as u8;
            }
        }
    }
    out
}
