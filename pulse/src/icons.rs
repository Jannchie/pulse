//! 进程图标：后台线程从 exe 提取，UI 线程按路径缓存成纹理。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use pulse_probe::icon::{self, Icon};

/// 提取尺寸：16pt 图标在 150%/200% 缩放下仍然清晰。
const SIZE: i32 = 32;

enum Slot {
    Pending,
    Ready(TextureHandle),
    Missing,
}

pub struct Icons {
    requests: Sender<Arc<str>>,
    results: Receiver<(Arc<str>, Option<Icon>)>,
    slots: HashMap<Arc<str>, Slot>,
}

impl Icons {
    pub fn new(ctx: &egui::Context) -> Self {
        let (req_tx, req_rx) = channel::<Arc<str>>();
        let (res_tx, res_rx) = channel();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("icons".into())
            .spawn(move || {
                for path in req_rx {
                    let icon = icon::extract(&path, SIZE);
                    if res_tx.send((path, icon)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("spawn icon worker");
        Self {
            requests: req_tx,
            results: res_rx,
            slots: HashMap::new(),
        }
    }

    /// 把后台提取完成的图标上传为纹理。每帧调用一次。
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((path, icon)) = self.results.try_recv() {
            let slot = match icon {
                Some(i) => Slot::Ready(ctx.load_texture(
                    path.as_ref(),
                    ColorImage::from_rgba_unmultiplied([i.size, i.size], &i.rgba),
                    TextureOptions::LINEAR,
                )),
                None => Slot::Missing,
            };
            self.slots.insert(path, slot);
        }
    }

    pub fn get(&mut self, path: &Arc<str>) -> Option<&TextureHandle> {
        let slot = self.slots.entry(path.clone()).or_insert_with(|| {
            let _ = self.requests.send(path.clone());
            Slot::Pending
        });
        match slot {
            Slot::Ready(t) => Some(t),
            Slot::Pending | Slot::Missing => None,
        }
    }
}
