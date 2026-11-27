use crate::ui::selection_scope::{SelectionScope, WidgetSelPlan};
use iced::advanced::text::{
    self,
    editor::{Action, Editor as _, Motion, Position, Selection},
    highlighter::{self, Highlighter},
    LineHeight, Wrapping,
};
use iced::advanced::widget::{tree, Tree, Widget};
use iced::advanced::{clipboard, layout, mouse, renderer, Clipboard, Layout, Shell};
use iced::{keyboard, window};
use iced::{Color, Element, Event, Font, Length, Pixels, Point, Rectangle, Size, Theme};
use std::cell::{Cell, RefCell};
use std::ops::Range;

pub type LineHighlights = Vec<Vec<(Range<usize>, highlighter::Format<Font>)>>;

struct SpanHighlighter
{
    lines: LineHighlights,
    current: usize,
}

impl Highlighter for SpanHighlighter
{
    type Settings = LineHighlights;
    type Highlight = highlighter::Format<Font>;
    type Iterator<'a> = std::vec::IntoIter<(Range<usize>, highlighter::Format<Font>)>;

    fn new(s: &Self::Settings) -> Self { Self { lines: s.clone(), current: 0 } }
    fn update(&mut self, s: &Self::Settings) { self.lines = s.clone(); self.current = 0; }
    fn change_line(&mut self, line: usize) { self.current = line; }
    fn current_line(&self) -> usize { self.current }
    fn highlight_line(&mut self, line: &str) -> Self::Iterator<'_>
    {
        let idx = self.current;
        self.current += 1;
        let out: Vec<_> = self.lines.get(idx).map(|row|
        {
            row.iter().filter(|(r, _)| r.start < r.end && r.end <= line.len() && line.is_char_boundary(r.start) && line.is_char_boundary(r.end)).cloned().collect()
        }).unwrap_or_default();
        out.into_iter()
    }
}

/// Converts full-string byte-range spans into per-line relative highlight ranges.
pub fn build_line_highlights(full_text: &str, spans: &[(Range<usize>, highlighter::Format<Font>)]) -> LineHighlights
{
    let mut off = 0usize;
    full_text.split('\n').map(|line|
    {
        let end_off = off + line.len();
        let mut row = Vec::new();
        for (r, fmt) in spans
        {
            if r.end <= off || r.start >= end_off { continue; }
            let (s, e) = (r.start.max(off) - off, r.end.min(end_off) - off);
            if s < e && line.is_char_boundary(s) && line.is_char_boundary(e) { row.push((s..e, *fmt)); }
        }
        off = end_off + 1;
        row
    }).collect()
}

fn global_byte_offset(source: &str, pos: Position) -> usize
{
    let mut off = 0usize;
    for (idx, line) in source.split('\n').enumerate()
    {
        if idx == pos.line { return off + pos.column.min(line.len()); }
        off += line.len() + 1;
    }
    source.len()
}

/// Read-only selectable multi-line text widget supporting mouse selection, Ctrl+C/A, and wheel pass-through.
pub struct SelectableText<'a, Message>
{
    content: String,
    highlights: LineHighlights,
    links: Vec<(Range<usize>, String)>,
    font: Font,
    size: Pixels,
    line_height: LineHeight,
    wrapping: Wrapping,
    text_color: Color,
    selection_color: Color,
    width: Length,
    scope: Option<SelectionScope>,
    nested_scroll: bool,
    on_link: Option<Box<dyn Fn(String) -> Message + 'a>>,
    on_copy: Option<Box<dyn Fn(String) -> Message + 'a>>,
}

impl<'a, Message> SelectableText<'a, Message>
{
    pub fn new(content: impl Into<String>) -> Self
    {
        Self { content: content.into().replace("\r\n", "\n"), highlights: Vec::new(), links: Vec::new(), font: Font::DEFAULT, size: Pixels(14.0), line_height: LineHeight::Relative(1.35), wrapping: Wrapping::WordOrGlyph, text_color: Color::WHITE, selection_color: Color::from_rgba(0.39, 0.40, 0.95, 0.30), width: Length::Fill, scope: None, nested_scroll: false, on_link: None, on_copy: None }
    }
    pub fn highlights(mut self, h: LineHighlights) -> Self { self.highlights = h; self }
    pub fn links(mut self, l: Vec<(Range<usize>, String)>) -> Self { self.links = l; self }
    pub fn font(mut self, font: Font) -> Self { self.font = font; self }
    pub fn size(mut self, size: impl Into<Pixels>) -> Self { self.size = size.into(); self }
    pub fn wrapping(mut self, wrapping: Wrapping) -> Self { self.wrapping = wrapping; self }
    pub fn color(mut self, color: Color) -> Self { self.text_color = color; self }
    pub fn selection_color(mut self, color: Color) -> Self { self.selection_color = color; self }
    pub fn width(mut self, width: impl Into<Length>) -> Self { self.width = width.into(); self }
    pub fn scope(mut self, scope: SelectionScope) -> Self { self.scope = Some(scope); self }
    pub fn nested_scroll(mut self, nested: bool) -> Self { self.nested_scroll = nested; self }
    pub fn on_link(mut self, f: impl Fn(String) -> Message + 'a) -> Self { self.on_link = Some(Box::new(f)); self }
    pub fn on_copy(mut self, f: impl Fn(String) -> Message + 'a) -> Self { self.on_copy = Some(Box::new(f)); self }
}

struct State<R: text::Renderer<Font = Font>>
{
    source: String,
    settings: LineHighlights,
    editor: RefCell<R::Editor>,
    highlighter: RefCell<SpanHighlighter>,
    focused: bool,
    synced_rev: Cell<u64>,
    last_click: Option<mouse::Click>,
    drag_kind: Option<mouse::click::Kind>,
}

impl<Message> SelectableText<'_, Message>
{
    fn sync_with_scope<R: text::Renderer<Font = Font>>(&self, st: &State<R>, bounds: Rectangle) -> bool
    {
        let Some(ref scope) = self.scope else { return false; };
        let (rev, plan) = scope.plan_for(bounds);
        if st.synced_rev.get() == rev { return false; }
        st.synced_rev.set(rev);
        let mut ed = st.editor.borrow_mut();
        match plan
        {
            WidgetSelPlan::None => ed.perform(Action::Move(Motion::DocumentStart)),
            WidgetSelPlan::All => ed.perform(Action::SelectAll),
            WidgetSelPlan::Single(a, b) =>
            {
                ed.perform(Action::Click(a));
                if (a.x - b.x).hypot(a.y - b.y) >= 1.0 { ed.perform(Action::Drag(b)); }
            }
            WidgetSelPlan::FromPointToEnd(p) => { ed.perform(Action::Click(p)); ed.perform(Action::Select(Motion::DocumentEnd)); }
            WidgetSelPlan::FromStartToPoint(p) => { ed.perform(Action::Move(Motion::DocumentStart)); ed.perform(Action::Drag(p)); }
            WidgetSelPlan::OriginWord(p) => { ed.perform(Action::Click(p)); ed.perform(Action::SelectWord); }
            WidgetSelPlan::OriginLine(p) => { ed.perform(Action::Click(p)); ed.perform(Action::SelectLine); }
        }
        if let Some(sel) = ed.copy() { scope.set_fragment(bounds, sel); }
        true
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer> for SelectableText<'_, Message>
where
    Renderer: text::Renderer<Font = Font> + 'static,
{
    fn tag(&self) -> tree::Tag { tree::Tag::of::<State<Renderer>>() }
    fn size(&self) -> Size<Length> { Size::new(self.width, Length::Shrink) }
    fn state(&self) -> tree::State
    {
        tree::State::new(State::<Renderer> { source: self.content.clone(), settings: self.highlights.clone(), editor: RefCell::new(Renderer::Editor::with_text(&self.content)), highlighter: RefCell::new(SpanHighlighter::new(&self.highlights)), focused: false, synced_rev: Cell::new(0), last_click: None, drag_kind: None })
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &Renderer, limits: &layout::Limits) -> layout::Node
    {
        let st = tree.state.downcast_mut::<State<Renderer>>();
        if st.source != self.content
        {
            *st.editor.borrow_mut() = Renderer::Editor::with_text(&self.content);
            st.source = self.content.clone();
            st.focused = false;
            st.synced_rev.set(0);
            st.drag_kind = None;
        }
        if st.settings != self.highlights || (st.source == self.content && st.highlighter.borrow().lines != self.highlights)
        {
            st.highlighter.borrow_mut().update(&self.highlights);
            st.settings = self.highlights.clone();
        }
        let limits = limits.width(self.width).height(Length::Shrink);
        let max_w = limits.max().width;
        let shape_w = if max_w.is_finite() { max_w.max(1.0) } else { 100_000.0 };
        st.editor.borrow_mut().update(Size::new(shape_w, f32::INFINITY), self.font, self.size, self.line_height, self.wrapping, &mut *st.highlighter.borrow_mut());
        let min = st.editor.borrow().min_bounds();
        layout::Node::new(limits.resolve(self.width, Length::Shrink, Size::new(min.width.max(1.0), min.height.max(self.size.0))))
    }

    fn update(&mut self, tree: &mut Tree, event: &Event, layout: Layout<'_>, cursor: mouse::Cursor, _r: &Renderer, cb: &mut dyn Clipboard, shell: &mut Shell<'_, Message>, _vp: &Rectangle)
    {
        let st = tree.state.downcast_mut::<State<Renderer>>();
        let bounds = layout.bounds();
        if let Some(ref scope) = self.scope
        {
            match event
            {
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) =>
                    if let (Some(local), Some(abs)) = (cursor.position_in(bounds), cursor.position())
                    {
                        let click = mouse::Click::new(local, mouse::Button::Left, st.last_click);
                        st.last_click = Some(click);
                        st.focused = true;
                        scope.begin(abs, bounds, click.kind());
                        self.sync_with_scope(st, bounds);
                        shell.capture_event();
                        shell.request_redraw();
                    }
                    else if st.focused { st.focused = false; shell.request_redraw(); }
                Event::Mouse(mouse::Event::CursorMoved { .. }) if scope.is_dragging() =>
                {
                    if let Some(abs) = cursor.position()
                    {
                        if !self.nested_scroll || cursor.position_in(bounds).is_some() { scope.update_drag(abs); }
                    }
                    if self.sync_with_scope(st, bounds) { shell.request_redraw(); }
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) =>
                {
                    let small = scope.is_small_click() && cursor.position_in(bounds).is_some();
                    let was_drag = scope.is_dragging();
                    scope.end_drag();
                    if self.sync_with_scope(st, bounds) || was_drag { shell.request_redraw(); }
                    if small && !self.links.is_empty()
                    {
                        let off = global_byte_offset(&st.source, st.editor.borrow().cursor().position);
                        if let (Some(on_link), Some((_, url))) = (&self.on_link, self.links.iter().find(|(r, _)| r.start <= off && off < r.end)) { shell.publish(on_link(url.clone())); }
                    }
                }
                Event::Window(window::Event::RedrawRequested(_)) =>
                    if self.sync_with_scope(st, bounds) { shell.request_redraw(); },
                Event::Keyboard(keyboard::Event::KeyPressed { key, physical_key, modifiers, .. }) =>
                {
                    if !shell.is_event_captured() && modifiers.command() && !modifiers.alt() && key.to_latin(*physical_key) == Some('c') && scope.has_selection()
                    {
                        self.sync_with_scope(st, bounds);
                        let sel = scope.copied_text();
                        if !sel.is_empty() { cb.write(clipboard::Kind::Standard, sel.clone()); if let Some(ref f) = self.on_copy { shell.publish(f(sel)); } shell.capture_event(); }
                    }
                    else if !shell.is_event_captured() && st.focused && modifiers.command() && !modifiers.alt() && key.to_latin(*physical_key) == Some('a')
                    {
                        st.editor.borrow_mut().perform(Action::SelectAll);
                        if let Some(sel) = st.editor.borrow().copy() { scope.set_fragment(bounds, sel); }
                        shell.capture_event();
                        shell.request_redraw();
                    }
                    else if matches!(key.as_ref(), keyboard::Key::Named(keyboard::key::Named::Escape)) && scope.clear()
                    {
                        self.sync_with_scope(st, bounds);
                        shell.request_redraw();
                    }
                }
                _ => {}
            }
            return;
        }
        match event
        {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) =>
            {
                if let Some(pos) = cursor.position_in(bounds)
                {
                    let click = mouse::Click::new(pos, mouse::Button::Left, st.last_click);
                    let act = match click.kind() { mouse::click::Kind::Single => Action::Click(pos), mouse::click::Kind::Double => Action::SelectWord, mouse::click::Kind::Triple => Action::SelectLine };
                    st.editor.borrow_mut().perform(act);
                    st.focused = true;
                    st.last_click = Some(click);
                    st.drag_kind = Some(click.kind());
                    shell.capture_event();
                    shell.request_redraw();
                }
                else if st.focused
                {
                    st.focused = false;
                    st.drag_kind = None;
                    st.editor.borrow_mut().perform(Action::Move(Motion::Right));
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) if st.drag_kind == Some(mouse::click::Kind::Single) =>
                if let Some(p) = cursor.position() { st.editor.borrow_mut().perform(Action::Drag(Point::new((p.x - bounds.x).clamp(0.0, bounds.width), (p.y - bounds.y).clamp(0.0, (bounds.height - 1.0).max(0.0))))); shell.request_redraw(); }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) =>
                if let Some(mouse::click::Kind::Single) = st.drag_kind.take()
                {
                    let ed = st.editor.borrow();
                    if ed.copy().is_none() && !self.links.is_empty()
                    {
                        let off = global_byte_offset(&st.source, ed.cursor().position);
                        if let (Some(on_link), Some((_, url))) = (&self.on_link, self.links.iter().find(|(r, _)| r.start <= off && off < r.end)) { shell.publish(on_link(url.clone())); }
                    }
                }
            Event::Keyboard(keyboard::Event::KeyPressed { key, physical_key, modifiers, .. }) if st.focused =>
            {
                if modifiers.command() && key.to_latin(*physical_key) == Some('c')
                {
                    if let Some(sel) = st.editor.borrow().copy() { cb.write(clipboard::Kind::Standard, sel.clone()); if let Some(ref f) = self.on_copy { shell.publish(f(sel)); } shell.capture_event(); }
                }
                else if modifiers.command() && key.to_latin(*physical_key) == Some('a') { st.editor.borrow_mut().perform(Action::SelectAll); shell.capture_event(); shell.request_redraw(); }
                else if matches!(key.as_ref(), keyboard::Key::Named(keyboard::key::Named::Escape)) { st.focused = false; st.editor.borrow_mut().perform(Action::Move(Motion::Right)); shell.request_redraw(); }
            }
            _ => {}
        }
    }

    fn draw(&self, tree: &Tree, renderer: &mut Renderer, _theme: &Theme, _defaults: &renderer::Style, layout: Layout<'_>, _c: mouse::Cursor, viewport: &Rectangle)
    {
        let bounds = layout.bounds();
        if !bounds.intersects(viewport) { return; }
        let st = tree.state.downcast_ref::<State<Renderer>>();
        self.sync_with_scope(st, bounds);
        let mut ed = st.editor.borrow_mut();
        if ed.is_empty() { return; }
        ed.highlight(self.font, &mut *st.highlighter.borrow_mut(), |fmt| *fmt);
        if st.focused || self.scope.as_ref().is_some_and(|s| s.is_active())
        {
            if let Selection::Range(ranges) = ed.selection()
            {
                let tr = bounds.position() - Point::ORIGIN;
                for r in ranges.into_iter().filter_map(|r| bounds.intersection(&(r + tr))) { renderer.fill_quad(renderer::Quad { bounds: r, ..Default::default() }, self.selection_color); }
            }
        }
        renderer.fill_editor(&*ed, bounds.position(), self.text_color, bounds);
    }

    fn mouse_interaction(&self, _t: &Tree, layout: Layout<'_>, cursor: mouse::Cursor, _v: &Rectangle, _r: &Renderer) -> mouse::Interaction
    {
        if cursor.is_over(layout.bounds()) { mouse::Interaction::Text } else { mouse::Interaction::None }
    }
}

impl<'a, Message: 'a> From<SelectableText<'a, Message>> for Element<'a, Message, Theme, iced::Renderer> { fn from(w: SelectableText<'a, Message>) -> Self { Element::new(w) } }

#[cfg(test)]
mod tests
{
    use super::*;

    #[test]
    fn line_highlights_split_utf8_across_lines()
    {
        let txt = "日本語のテスト\n2行目コード";
        let hl = build_line_highlights(txt, &[(txt.find("テスト").unwrap()..txt.find("コード").unwrap(), highlighter::Format { color: Some(Color::WHITE), font: None })]);
        assert_eq!((&"日本語のテスト"[hl[0][0].0.clone()], &"2行目コード"[hl[1][0].0.clone()]), ("テスト", "2行目"));
    }
}
