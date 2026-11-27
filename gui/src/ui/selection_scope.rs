use iced::advanced::mouse::click;
use iced::{Point, Rectangle};
use parking_lot::Mutex;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WidgetSelPlan
{
    None,
    All,
    Single(Point, Point),
    FromPointToEnd(Point),
    FromStartToPoint(Point),
    OriginWord(Point),
    OriginLine(Point),
}

fn clamp_local(bounds: Rectangle, p: Point) -> Point
{
    Point::new(
        (p.x - bounds.x).clamp(0.0, bounds.width),
        (p.y - bounds.y).clamp(0.0, (bounds.height - 1.0).max(0.0)),
    )
}

pub fn plan_widget_selection(bounds: Rectangle, anchor: Point, current: Point) -> WidgetSelPlan
{
    if bounds.width <= 0.0 || bounds.height <= 0.0 { return WidgetSelPlan::None; }
    let in_y = |p: Point| p.y >= bounds.y && p.y < bounds.y + bounds.height;
    if in_y(anchor) && in_y(current)
    {
        let (left_x, right_x) = if anchor.x <= current.x { (anchor.x, current.x) } else { (current.x, anchor.x) };
        if right_x < bounds.x || left_x > bounds.x + bounds.width { return WidgetSelPlan::None; }
        if left_x < bounds.x && right_x > bounds.x + bounds.width { return WidgetSelPlan::All; }
        return WidgetSelPlan::Single(clamp_local(bounds, anchor), clamp_local(bounds, current));
    }
    let (top, bot) = if anchor.y <= current.y { (anchor, current) } else { (current, anchor) };
    if bounds.y + bounds.height <= top.y || bounds.y >= bot.y { return WidgetSelPlan::None; }
    if in_y(top)
    {
        if bounds.x + bounds.width < top.x { return WidgetSelPlan::None; }
        if bounds.x > top.x { return WidgetSelPlan::All; }
        return WidgetSelPlan::FromPointToEnd(clamp_local(bounds, top));
    }
    if in_y(bot)
    {
        if bounds.x > bot.x { return WidgetSelPlan::None; }
        if bounds.x + bounds.width < bot.x { return WidgetSelPlan::All; }
        return WidgetSelPlan::FromStartToPoint(clamp_local(bounds, bot));
    }
    WidgetSelPlan::All
}

#[derive(Default)]
struct SelectionScopeInner
{
    revision: u64,
    active: bool,
    dragging: bool,
    kind: Option<click::Kind>,
    anchor: Point,
    current: Point,
    origin_bounds: Option<Rectangle>,
    fragments: Vec<(i32, i32, String)>,
}

#[derive(Clone, Default)]
pub struct SelectionScope(Arc<Mutex<SelectionScopeInner>>);

impl SelectionScope
{
    pub fn begin(&self, pos: Point, bounds: Rectangle, kind: click::Kind)
    {
        let mut g = self.0.lock();
        g.revision = g.revision.wrapping_add(1).max(1);
        g.active = true;
        g.dragging = kind == click::Kind::Single;
        g.kind = Some(kind);
        g.anchor = pos;
        g.current = pos;
        g.origin_bounds = Some(bounds);
        g.fragments.clear();
    }

    pub fn update_drag(&self, pos: Point) -> bool
    {
        let mut g = self.0.lock();
        if !g.dragging { return false; }
        if (pos.x - g.current.x).abs() <= 0.25 && (pos.y - g.current.y).abs() <= 0.25 { return false; }
        g.current = pos;
        g.revision = g.revision.wrapping_add(1).max(1);
        g.fragments.clear();
        true
    }

    pub fn end_drag(&self)
    {
        self.0.lock().dragging = false;
    }

    pub fn clear(&self) -> bool
    {
        let mut g = self.0.lock();
        if !g.active && !g.dragging && g.fragments.is_empty() { return false; }
        g.active = false;
        g.dragging = false;
        g.kind = None;
        g.origin_bounds = None;
        g.fragments.clear();
        g.revision = g.revision.wrapping_add(1).max(1);
        true
    }

    pub fn is_active(&self) -> bool { self.0.lock().active }

    pub fn is_dragging(&self) -> bool { self.0.lock().dragging }

    pub fn is_small_click(&self) -> bool
    {
        let g = self.0.lock();
        g.active && g.kind == Some(click::Kind::Single) && (g.anchor.x - g.current.x).hypot(g.anchor.y - g.current.y) < 3.0
    }

    pub fn plan_for(&self, bounds: Rectangle) -> (u64, WidgetSelPlan)
    {
        let g = self.0.lock();
        if !g.active { return (g.revision, WidgetSelPlan::None); }
        let plan = match g.kind
        {
            Some(click::Kind::Double) => if g.origin_bounds == Some(bounds) { WidgetSelPlan::OriginWord(clamp_local(bounds, g.anchor)) } else { WidgetSelPlan::None },
            Some(click::Kind::Triple) => if g.origin_bounds == Some(bounds) { WidgetSelPlan::OriginLine(clamp_local(bounds, g.anchor)) } else { WidgetSelPlan::None },
            _ => plan_widget_selection(bounds, g.anchor, g.current),
        };
        (g.revision, plan)
    }

    pub fn set_fragment(&self, bounds: Rectangle, text: String)
    {
        if text.is_empty() { return; }
        let (ky, kx) = (bounds.y.round() as i32, bounds.x.round() as i32);
        let mut g = self.0.lock();
        if let Some(slot) = g.fragments.iter_mut().find(|(y, x, _)| *y == ky && *x == kx) { slot.2 = text; }
        else { g.fragments.push((ky, kx, text)); }
    }

    pub fn has_selection(&self) -> bool
    {
        let g = self.0.lock();
        g.active && g.fragments.iter().any(|(_, _, t)| !t.is_empty())
    }

    pub fn copied_text(&self) -> String
    {
        let g = self.0.lock();
        if !g.active || g.fragments.is_empty() { return String::new(); }
        let mut items: Vec<_> = g.fragments.iter().filter(|(_, _, t)| !t.is_empty()).cloned().collect();
        items.sort_by_key(|(y, x, _)| (*y, *x));
        let mut out = String::new();
        let mut prev_y: Option<i32> = None;
        for (y, _, txt) in items
        {
            if let Some(py) = prev_y { out.push_str(if (y - py).abs() <= 4 { "\t" } else { "\n" }); }
            out.push_str(&txt);
            prev_y = Some(y);
        }
        out
    }
}

#[cfg(test)]
mod tests
{
    use super::*;
    use iced::Size;

    #[test]
    fn plans_cross_block_and_table_selection_both_directions()
    {
        let p1 = Rectangle::new(Point::new(20.0, 100.0), Size::new(300.0, 24.0));
        let p2 = Rectangle::new(Point::new(20.0, 132.0), Size::new(300.0, 24.0));
        let p3 = Rectangle::new(Point::new(20.0, 164.0), Size::new(300.0, 24.0));
        let a = Point::new(80.0, 110.0);
        let b = Point::new(150.0, 174.0);
        assert_eq!(plan_widget_selection(p1, a, b), WidgetSelPlan::FromPointToEnd(Point::new(60.0, 10.0)));
        assert_eq!(plan_widget_selection(p2, a, b), WidgetSelPlan::All);
        assert_eq!(plan_widget_selection(p3, a, b), WidgetSelPlan::FromStartToPoint(Point::new(130.0, 10.0)));
        assert_eq!(plan_widget_selection(p1, b, a), WidgetSelPlan::FromPointToEnd(Point::new(60.0, 10.0)));
        assert_eq!(plan_widget_selection(p2, b, a), WidgetSelPlan::All);
        assert_eq!(plan_widget_selection(p3, b, a), WidgetSelPlan::FromStartToPoint(Point::new(130.0, 10.0)));

        let c1 = Rectangle::new(Point::new(20.0, 200.0), Size::new(100.0, 22.0));
        let c2 = Rectangle::new(Point::new(130.0, 200.0), Size::new(100.0, 22.0));
        assert_eq!(plan_widget_selection(c1, Point::new(140.0, 210.0), Point::new(180.0, 210.0)), WidgetSelPlan::None);
        assert_eq!(plan_widget_selection(c2, Point::new(140.0, 210.0), Point::new(180.0, 210.0)), WidgetSelPlan::Single(Point::new(10.0, 10.0), Point::new(50.0, 10.0)));
    }

    #[test]
    fn scope_orders_fragments_tables_and_clears()
    {
        let scope = SelectionScope::default();
        let b1 = Rectangle::new(Point::new(20.0, 100.0), Size::new(200.0, 20.0));
        let b2 = Rectangle::new(Point::new(20.0, 130.0), Size::new(90.0, 20.0));
        let b3 = Rectangle::new(Point::new(120.0, 130.0), Size::new(90.0, 20.0));
        scope.begin(Point::new(30.0, 108.0), b1, click::Kind::Single);
        scope.set_fragment(b3, "C2".into());
        scope.set_fragment(b1, "Heading".into());
        scope.set_fragment(b2, "C1".into());
        assert_eq!(scope.copied_text(), "Heading\nC1\tC2");
        assert!(scope.has_selection());
        assert!(scope.clear());
        assert!(!scope.has_selection());
        assert_eq!(scope.copied_text(), "");
    }
}
