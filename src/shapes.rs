//! 标注图形模块：选区与矢量标注的数据定义（坐标系均为全图图片像素）。
//!
//! UI（`overlay`）只操作这些数据，落盘时的栅格化由 `export` 负责，
//! 两边共用同一套坐标约定，避免换算分歧。

/// 图片像素坐标系下的矩形：左上角 `(x, y)` 加宽高。
#[derive(Clone, Copy, Debug, Default)]
pub struct PixelRect {
    /// 左上角横坐标（图片像素）。
    pub x: f32,
    /// 左上角纵坐标（图片像素）。
    pub y: f32,
    /// 宽度（图片像素）。
    pub w: f32,
    /// 高度（图片像素）。
    pub h: f32,
}

impl PixelRect {
    /// 构造矩形。
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// 由任意两个对角点构造规范化矩形（宽高非负）。
    pub fn from_points(a: (f32, f32), b: (f32, f32)) -> Self {
        Self {
            x: a.0.min(b.0),
            y: a.1.min(b.1),
            w: (a.0 - b.0).abs(),
            h: (a.1 - b.1).abs(),
        }
    }

    /// 矩形是否退化为空（宽或高为 0）。
    pub fn is_empty(&self) -> bool {
        self.w <= 0.5 || self.h <= 0.5
    }

    /// 点是否落在矩形内（含边界）。
    pub fn contains(&self, p: (f32, f32)) -> bool {
        p.0 >= self.x && p.0 <= self.x + self.w && p.1 >= self.y && p.1 <= self.y + self.h
    }
}

/// 标注颜色：RGBA 各分量 0-255（预设比较用，派生判等）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnnoColor(pub u8, pub u8, pub u8, pub u8);

impl AnnoColor {
    /// 预设红（默认标注色，对标 Snipaste）。
    pub const RED: Self = Self(231, 76, 60, 255);
    /// 预设黄。
    pub const YELLOW: Self = Self(241, 196, 15, 255);
    /// 预设绿。
    pub const GREEN: Self = Self(46, 204, 113, 255);
    /// 预设蓝。
    pub const BLUE: Self = Self(52, 152, 219, 255);
    /// 预设黑。
    pub const BLACK: Self = Self(30, 30, 30, 255);
    /// 预设白。
    pub const WHITE: Self = Self(255, 255, 255, 255);
    /// 预设橙。
    pub const ORANGE: Self = Self(230, 126, 34, 255);
    /// 预设青。
    pub const CYAN: Self = Self(0, 188, 212, 255);

    /// 转为 rgpui 画布颜色（`paint_path` / 文字颜色用）。
    pub fn to_rgpui(self) -> rgpui::Rgba {
        rgpui::rgba(
            ((self.0 as u32) << 24)
                | ((self.1 as u32) << 16)
                | ((self.2 as u32) << 8)
                | (self.3 as u32),
        )
    }
}

/// 浮层上的矢量标注，坐标均为全图图片像素。
#[derive(Clone, Debug)]
pub enum Shape {
    /// 自由画笔：一串连续点。
    Pen {
        /// 笔迹点列。
        pts: Vec<(f32, f32)>,
        /// 颜色。
        color: AnnoColor,
        /// 线宽（图片像素）。
        width: f32,
    },
    /// 荧光笔：半透明粗线（颜色 alpha 会被强制压到约 40%）。
    Highlighter {
        /// 笔迹点列。
        pts: Vec<(f32, f32)>,
        /// 颜色。
        color: AnnoColor,
        /// 线宽（图片像素）。
        width: f32,
    },
    /// 矩形框。
    Rect {
        /// 矩形区域。
        rect: PixelRect,
        /// 颜色。
        color: AnnoColor,
        /// 线宽（图片像素）。
        width: f32,
    },
    /// 椭圆框（外接矩形确定）。
    Ellipse {
        /// 外接矩形。
        rect: PixelRect,
        /// 颜色。
        color: AnnoColor,
        /// 线宽（图片像素）。
        width: f32,
    },
    /// 箭头：起点到终点，头部自动生成。
    Arrow {
        /// 起点。
        from: (f32, f32),
        /// 终点（箭头方向）。
        to: (f32, f32),
        /// 颜色。
        color: AnnoColor,
        /// 线宽（图片像素）。
        width: f32,
    },
    /// 马赛克：矩形区域打码。
    Mosaic {
        /// 打码区域。
        rect: PixelRect,
    },
    /// 文字：点击落点 + 单行文本。
    Text {
        /// 落点（文本左上角附近）。
        pos: (f32, f32),
        /// 文本内容。
        content: String,
        /// 颜色。
        color: AnnoColor,
        /// 字号（图片像素，近似字高）。
        size: f32,
    },
}

impl Shape {
    /// 标注的外包矩形（橡皮擦命中测试用）。
    pub fn bbox(&self) -> PixelRect {
        match self {
            // 点列取极值并向外扩展线宽，避免细线点不中。
            Self::Pen { pts, width, .. } | Self::Highlighter { pts, width, .. } => {
                let mut min_x = f32::MAX;
                let mut min_y = f32::MAX;
                let mut max_x = f32::MIN;
                let mut max_y = f32::MIN;
                for (x, y) in pts {
                    min_x = min_x.min(*x);
                    min_y = min_y.min(*y);
                    max_x = max_x.max(*x);
                    max_y = max_y.max(*y);
                }
                let pad = *width;
                PixelRect::new(
                    min_x - pad,
                    min_y - pad,
                    (max_x - min_x) + pad * 2.0,
                    (max_y - min_y) + pad * 2.0,
                )
            }
            Self::Rect { rect, width, .. } | Self::Ellipse { rect, width, .. } => PixelRect::new(
                rect.x - *width,
                rect.y - *width,
                rect.w + width * 2.0,
                rect.h + width * 2.0,
            ),
            Self::Arrow {
                from, to, width, ..
            } => {
                let rect = PixelRect::from_points(*from, *to);
                PixelRect::new(
                    rect.x - *width,
                    rect.y - *width,
                    rect.w + width * 2.0,
                    rect.h + width * 2.0,
                )
            }
            Self::Mosaic { rect } => *rect,
            // 文本按“字数 × 字号”估算宽度，命中测试够用即可。
            Self::Text {
                pos, content, size, ..
            } => PixelRect::new(
                pos.0,
                pos.1,
                content.chars().count() as f32 * size * 0.9,
                size * 1.3,
            ),
        }
        .normalized_for_hit_test()
    }
}

impl PixelRect {
    /// 命中测试用规范化：宽高至少为 1，避免空矩形永远点不中。
    fn normalized_for_hit_test(mut self) -> Self {
        if self.w < 1.0 {
            self.w = 1.0;
        }
        if self.h < 1.0 {
            self.h = 1.0;
        }
        self
    }
}
