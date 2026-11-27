use crate::ui::theme::UiPalette;
use iced::border;
use iced::mouse;
use iced::widget::canvas::{self, gradient, Canvas, Frame, Geometry, Path, Stroke};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};
use std::collections::HashMap;
use std::f32::consts::TAU;
use std::time::Instant;

#[derive(Clone, Copy, Debug)]
pub struct EaseAnim {
    pub from: f32,
    pub to: f32,
    pub current: f32,
    pub elapsed_ms: f32,
    pub duration_ms: f32,
}

impl EaseAnim {
    pub fn new(initial: f32) -> Self {
        Self {
            from: initial,
            to: initial,
            current: initial,
            elapsed_ms: 0.0,
            duration_ms: 0.0,
        }
    }

    pub fn set_target(&mut self, target: f32, duration_ms: f32) {
        if (self.to - target).abs() < 0.0005 {
            return;
        }
        self.from = self.current;
        self.to = target;
        self.elapsed_ms = 0.0;
        self.duration_ms = duration_ms.max(1.0);
    }

    pub fn tick(&mut self, dt_ms: f32) -> bool {
        if !self.is_active() {
            return false;
        }
        self.elapsed_ms = (self.elapsed_ms + dt_ms).min(self.duration_ms);
        let t = (self.elapsed_ms / self.duration_ms).clamp(0.0, 1.0);
        let eased = 1.0 - (1.0 - t).powi(3);
        self.current = self.from + (self.to - self.from) * eased;
        if t >= 1.0 {
            self.current = self.to;
        }
        true
    }

    pub fn value(&self) -> f32 {
        self.current
    }

    pub fn is_active(&self) -> bool {
        self.duration_ms > 0.0
            && self.elapsed_ms < self.duration_ms
            && (self.current - self.to).abs() > 0.0005
    }

    pub fn is_visible(&self) -> bool {
        self.current > 0.015 || self.to > 0.5
    }
}

#[derive(Clone, Debug)]
pub struct ToastItem {
    pub level: String,
    pub message: String,
    pub age_ms: f32,
    pub hold_ms: f32,
}

impl ToastItem {
    pub fn new(level: &str, message: &str) -> Self {
        Self {
            level: level.to_string(),
            message: message.to_string(),
            age_ms: 0.0,
            hold_ms: 3200.0,
        }
    }

    pub fn is_expired(&self) -> bool {
        self.age_ms >= 440.0 + self.hold_ms
    }

    pub fn opacity_and_offset(&self) -> (f32, f32) {
        if self.age_ms < 220.0 {
            let t = (self.age_ms / 220.0).clamp(0.0, 1.0);
            let e = 1.0 - (1.0 - t).powi(3);
            (e, 18.0 * (1.0 - e))
        } else if self.age_ms <= 220.0 + self.hold_ms {
            (1.0, 0.0)
        } else {
            let t = ((self.age_ms - 220.0 - self.hold_ms) / 220.0).clamp(0.0, 1.0);
            let e = t * t;
            ((1.0 - e).max(0.0), 28.0 * e)
        }
    }
}

pub struct UiAnimations {
    pub sidebar: EaseAnim,
    pub drawer: EaseAnim,
    pub drawer_width: EaseAnim,
    pub find_bar: EaseAnim,
    pub task_strip: EaseAnim,
    pub task_body: EaseAnim,
    pub notice: EaseAnim,
    pub question: EaseAnim,
    pub attach_tray: EaseAnim,
    pub slash_menu: EaseAnim,
    pub modal: EaseAnim,
    pub drop_overlay: EaseAnim,
    pub loading_bar: EaseAnim,
    pub accordions: HashMap<String, EaseAnim>,
    pub toasts: Vec<ToastItem>,
    pub shimmer_phase: f32,
    pub pulse_phase: f32,
    pub last_tick: Option<Instant>,
}

impl Default for UiAnimations {
    fn default() -> Self {
        Self {
            sidebar: EaseAnim::new(1.0),
            drawer: EaseAnim::new(0.0),
            drawer_width: EaseAnim::new(390.0),
            find_bar: EaseAnim::new(0.0),
            task_strip: EaseAnim::new(0.0),
            task_body: EaseAnim::new(1.0),
            notice: EaseAnim::new(0.0),
            question: EaseAnim::new(0.0),
            attach_tray: EaseAnim::new(0.0),
            slash_menu: EaseAnim::new(0.0),
            modal: EaseAnim::new(0.0),
            drop_overlay: EaseAnim::new(0.0),
            loading_bar: EaseAnim::new(0.0),
            accordions: HashMap::new(),
            toasts: Vec::new(),
            shimmer_phase: 0.0,
            pulse_phase: 0.0,
            last_tick: None,
        }
    }
}

impl UiAnimations {
    pub fn push_toast(&mut self, level: &str, message: &str) {
        if message.trim().is_empty() {
            return;
        }
        self.toasts.push(ToastItem::new(level, message));
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
    }

    pub fn toggle_accordion(&mut self, key: &str, default_open: bool) {
        let entry = self
            .accordions
            .entry(key.to_string())
            .or_insert_with(|| EaseAnim::new(if default_open { 1.0 } else { 0.0 }));
        let next = if entry.to > 0.5 { 0.0 } else { 1.0 };
        entry.set_target(next, 210.0);
    }

    pub fn accordion_value(&self, key: &str, default_open: bool) -> f32 {
        self.accordions
            .get(key)
            .map(|a| a.value())
            .unwrap_or(if default_open { 1.0 } else { 0.0 })
    }

    pub fn needs_frame(&self, show_shimmer_or_pulse: bool) -> bool {
        if show_shimmer_or_pulse || !self.toasts.is_empty() {
            return true;
        }
        let core_active = self.sidebar.is_active()
            || self.drawer.is_active()
            || self.drawer_width.is_active()
            || self.find_bar.is_active()
            || self.task_strip.is_active()
            || self.task_body.is_active()
            || self.notice.is_active()
            || self.question.is_active()
            || self.attach_tray.is_active()
            || self.slash_menu.is_active()
            || self.modal.is_active()
            || self.drop_overlay.is_active()
            || self.loading_bar.is_active();
        core_active || self.accordions.values().any(|a| a.is_active())
    }

    pub fn tick(&mut self, now: Instant, advance_cycles: bool) {
        let dt_ms = self
            .last_tick
            .map(|prev| now.saturating_duration_since(prev).as_secs_f32() * 1000.0)
            .unwrap_or(16.6)
            .clamp(1.0, 50.0);
        self.last_tick = Some(now);

        self.sidebar.tick(dt_ms);
        self.drawer.tick(dt_ms);
        self.drawer_width.tick(dt_ms);
        self.find_bar.tick(dt_ms);
        self.task_strip.tick(dt_ms);
        self.task_body.tick(dt_ms);
        self.notice.tick(dt_ms);
        self.question.tick(dt_ms);
        self.attach_tray.tick(dt_ms);
        self.slash_menu.tick(dt_ms);
        self.modal.tick(dt_ms);
        self.drop_overlay.tick(dt_ms);
        self.loading_bar.tick(dt_ms);
        for anim in self.accordions.values_mut() {
            anim.tick(dt_ms);
        }
        for toast in &mut self.toasts {
            toast.age_ms += dt_ms;
        }
        self.toasts.retain(|t| !t.is_expired());

        if advance_cycles {
            self.shimmer_phase = (self.shimmer_phase + dt_ms / 1350.0) % 1.0;
            self.pulse_phase = (self.pulse_phase + dt_ms / 1000.0 * TAU * 0.72) % TAU;
        }
    }
}

pub struct TopProgressBar {
    pub phase: f32,
    pub intensity: f32,
    pub pal: UiPalette,
}

impl<M> canvas::Program<M> for TopProgressBar {
    type State = ();

    fn draw(
        &self,
        _: &(),
        renderer: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        if self.intensity <= 0.01 {
            return vec![];
        }
        frame.fill_rectangle(
            Point::ORIGIN,
            bounds.size(),
            self.pal.accent_soft.scale_alpha(self.intensity),
        );
        let w = bounds.width.max(1.0);
        let bar_w = (w * 0.34).clamp(110.0, 320.0);
        let start = -bar_w + (w + bar_w * 1.4) * self.phase;
        let grad = gradient::Linear::new(Point::new(start, 0.0), Point::new(start + bar_w, 0.0))
            .add_stop(0.0, self.pal.accent.scale_alpha(0.0))
            .add_stop(0.35, self.pal.accent.scale_alpha(self.intensity))
            .add_stop(0.65, self.pal.accent_hover.scale_alpha(self.intensity))
            .add_stop(1.0, self.pal.accent_hover.scale_alpha(0.0));
        frame.fill_rectangle(Point::new(start, 0.0), Size::new(bar_w, bounds.height), grad);
        vec![frame.into_geometry()]
    }
}

pub fn top_progress_bar<'a, M: 'a>(phase: f32, intensity: f32, pal: UiPalette) -> Element<'a, M> {
    if intensity <= 0.01 {
        return iced::widget::space().height(0).into();
    }
    let h = (3.0 * intensity).max(0.0);
    Canvas::new(TopProgressBar {
        phase,
        intensity,
        pal,
    })
    .width(Length::Fill)
    .height(Length::Fixed(h))
    .into()
}

pub struct ShimmerSkeleton {
    pub phase: f32,
    pub rows: Vec<(f32, f32)>,
    pub gap: f32,
    pub radius: f32,
    pub pal: UiPalette,
}

impl<M> canvas::Program<M> for ShimmerSkeleton {
    type State = ();

    fn draw(
        &self,
        _: &(),
        renderer: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let w = bounds.width.max(40.0);
        let base_c = if self.pal.dark {
            Color::from_rgba(1.0, 1.0, 1.0, 0.05)
        } else {
            Color::from_rgba(0.06, 0.09, 0.16, 0.06)
        };
        let gleam_c = if self.pal.dark {
            Color::from_rgba(0.62, 0.65, 1.0, 0.14)
        } else {
            Color::from_rgba(0.35, 0.38, 0.95, 0.13)
        };
        let sweep_w = (w * 0.45).max(80.0);
        let sweep_x = -sweep_w + (w + sweep_w * 1.5) * self.phase;

        let mut y = 0.0;
        for (frac, rh) in &self.rows {
            if y + *rh > bounds.height + 1.0 {
                break;
            }
            let rw = (w * frac.clamp(0.15, 1.0)).max(24.0);
            let rect = Path::rounded_rectangle(
                Point::new(0.0, y),
                Size::new(rw, *rh),
                border::Radius::from(self.radius),
            );
            frame.fill(&rect, base_c);

            let gleam_left = sweep_x.max(0.0);
            let gleam_right = (sweep_x + sweep_w).min(rw);
            if gleam_right > gleam_left {
                let g = gradient::Linear::new(Point::new(gleam_left, y), Point::new(gleam_right, y + *rh * 0.25))
                    .add_stop(0.0, gleam_c.scale_alpha(0.0))
                    .add_stop(0.5, gleam_c)
                    .add_stop(1.0, gleam_c.scale_alpha(0.0));
                let g_path = Path::rounded_rectangle(
                    Point::new(gleam_left, y),
                    Size::new(gleam_right - gleam_left, *rh),
                    border::Radius::from(self.radius),
                );
                frame.fill(&g_path, g);
            }
            y += *rh + self.gap;
        }
        vec![frame.into_geometry()]
    }
}

pub fn shimmer_skeleton<'a, M: 'a>(
    phase: f32,
    rows: Vec<(f32, f32)>,
    gap: f32,
    radius: f32,
    pal: UiPalette,
) -> Element<'a, M> {
    let total_h: f32 = rows.iter().map(|(_, h)| *h).sum::<f32>()
        + gap * rows.len().saturating_sub(1) as f32;
    Canvas::new(ShimmerSkeleton {
        phase,
        rows,
        gap,
        radius,
        pal,
    })
    .width(Length::Fill)
    .height(Length::Fixed(total_h.max(12.0)))
    .into()
}

pub struct ThinkingWave {
    pub phase: f32,
    pub color: Color,
}

impl<M> canvas::Program<M> for ThinkingWave {
    type State = ();

    fn draw(
        &self,
        _: &(),
        renderer: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let cy = bounds.height * 0.5;
        for i in 0..3 {
            let t = self.phase - (i as f32) * 0.85;
            let wave = (t.sin() + 1.0) * 0.5;
            let r = 3.0 + wave * 1.4;
            let alpha = 0.35 + wave * 0.65;
            let y = cy - wave * 3.0;
            let c = Path::circle(Point::new(6.0 + (i as f32) * 11.0, y), r);
            frame.fill(&c, self.color.scale_alpha(alpha));
        }
        vec![frame.into_geometry()]
    }
}

pub fn thinking_dots<'a, M: 'a>(phase: f32, color: Color) -> Element<'a, M> {
    Canvas::new(ThinkingWave { phase, color })
        .width(Length::Fixed(36.0))
        .height(Length::Fixed(18.0))
        .into()
}

pub struct PulseDot {
    pub phase: f32,
    pub color: Color,
    pub animated: bool,
}

impl<M> canvas::Program<M> for PulseDot {
    type State = ();

    fn draw(
        &self,
        _: &(),
        renderer: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let center = frame.center();
        if self.animated {
            let wave = (self.phase.sin() + 1.0) * 0.5;
            let halo = Path::circle(center, 3.8 + wave * 3.0);
            frame.fill(&halo, self.color.scale_alpha(0.32 - wave * 0.22));
        }
        let core = Path::circle(center, 3.5);
        frame.fill(&core, self.color);
        vec![frame.into_geometry()]
    }
}

pub fn pulse_dot<'a, M: 'a>(phase: f32, color: Color, animated: bool) -> Element<'a, M> {
    Canvas::new(PulseDot {
        phase,
        color,
        animated,
    })
    .width(Length::Fixed(14.0))
    .height(Length::Fixed(14.0))
    .into()
}

pub struct ChevronIcon {
    pub progress: f32,
    pub color: Color,
}

impl<M> canvas::Program<M> for ChevronIcon {
    type State = ();

    fn draw(
        &self,
        _: &(),
        renderer: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let center = frame.center();
        let angle = self.progress * std::f32::consts::FRAC_PI_2;
        let cos = angle.cos();
        let sin = angle.sin();
        let rot = |dx: f32, dy: f32| Point::new(center.x + dx * cos - dy * sin, center.y + dx * sin + dy * cos);
        let p = Path::new(|b| {
            b.move_to(rot(-2.2, -3.8));
            b.line_to(rot(2.2, 0.0));
            b.line_to(rot(-2.2, 3.8));
        });
        frame.stroke(
            &p,
            Stroke::default().with_color(self.color).with_width(1.6),
        );
        vec![frame.into_geometry()]
    }
}

pub fn chevron_icon<'a, M: 'a>(progress: f32, color: Color) -> Element<'a, M> {
    Canvas::new(ChevronIcon { progress, color })
        .width(Length::Fixed(14.0))
        .height(Length::Fixed(14.0))
        .into()
}

pub struct MeterBar {
    pub ratio: f32,
    pub pal: UiPalette,
}

impl<M> canvas::Program<M> for MeterBar {
    type State = ();

    fn draw(
        &self,
        _: &(),
        renderer: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let track = Path::rounded_rectangle(Point::ORIGIN, bounds.size(), 99.0.into());
        frame.fill(&track, self.pal.bg_canvas);
        let r = self.ratio.clamp(0.0, 1.0);
        if r > 0.01 {
            let fill_w = (bounds.width * r).max(bounds.height);
            let bar = Path::rounded_rectangle(
                Point::ORIGIN,
                Size::new(fill_w.min(bounds.width), bounds.height),
                99.0.into(),
            );
            let grad = gradient::Linear::new(Point::ORIGIN, Point::new(fill_w.min(bounds.width), 0.0))
                .add_stop(0.0, self.pal.accent)
                .add_stop(1.0, self.pal.accent_hover);
            frame.fill(&bar, grad);
        }
        vec![frame.into_geometry()]
    }
}

pub fn meter_bar<'a, M: 'a>(ratio: f32, pal: UiPalette) -> Element<'a, M> {
    Canvas::new(MeterBar { ratio, pal })
        .width(Length::Fill)
        .height(Length::Fixed(8.0))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_anim_interpolates_and_settles() {
        let mut a = EaseAnim::new(0.0);
        a.set_target(1.0, 200.0);
        assert!(a.is_active());
        a.tick(50.0);
        assert!(a.value() > 0.0 && a.value() < 1.0);
        for _ in 0..10 {
            a.tick(30.0);
        }
        assert!((a.value() - 1.0).abs() < 0.001);
        assert!(!a.is_active());
    }

    #[test]
    fn ui_animations_idle_vs_active_and_toast_lifecycle() {
        let mut anims = UiAnimations::default();
        assert!(!anims.needs_frame(false));
        anims.push_toast("ok", "hello");
        assert!(anims.needs_frame(false));
        let (alpha0, dx0) = anims.toasts[0].opacity_and_offset();
        assert!(alpha0 < 0.01 && dx0 > 10.0);
        for _ in 0..10 {
            anims.tick(Instant::now(), false);
        }
        anims.toasts[0].age_ms = 5000.0;
        anims.tick(Instant::now(), false);
        assert!(anims.toasts.is_empty());
    }
}
