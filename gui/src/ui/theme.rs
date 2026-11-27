use iced::border;
use iced::theme::palette as iced_palette;
use iced::widget::{
    button, container, overlay::menu, pick_list, rule, scrollable, slider, text_editor, text_input,
    toggler,
};
use iced::{font, Background, Border, Color, Font, Shadow, Theme, Vector};

#[cfg(windows)]
pub const UI_FONT: Font = Font::with_name("Yu Gothic UI");
#[cfg(not(windows))]
pub const UI_FONT: Font = Font::DEFAULT;
pub const UI_FONT_BOLD: Font = Font {
    weight: font::Weight::Bold,
    ..UI_FONT
};
pub const UI_FONT_ITALIC: Font = Font {
    style: font::Style::Italic,
    ..UI_FONT
};
#[cfg(windows)]
pub const MONO_FONT: Font = Font::with_name("Cascadia Code");
#[cfg(not(windows))]
pub const MONO_FONT: Font = Font::MONOSPACE;
pub const MONO_FONT_BOLD: Font = Font {
    weight: font::Weight::Bold,
    ..MONO_FONT
};

#[derive(Clone, Copy, Debug)]
pub struct UiPalette {
    pub dark: bool,
    pub bg_canvas: Color,
    pub bg_sidebar: Color,
    pub bg_surface: Color,
    pub bg_surface_2: Color,
    pub bg_surface_hover: Color,
    pub bg_glass: Color,
    pub bg_code: Color,
    pub bg_user_bubble: Color,
    pub border_subtle: Color,
    pub border_strong: Color,
    pub ink: Color,
    pub ink_secondary: Color,
    pub ink_muted: Color,
    pub accent: Color,
    pub accent_hover: Color,
    pub accent_soft: Color,
    pub accent_border: Color,
    pub green: Color,
    pub green_soft: Color,
    pub amber: Color,
    pub amber_soft: Color,
    pub red: Color,
    pub red_soft: Color,
    pub diff_add_bg: Color,
    pub diff_del_bg: Color,
    pub tok_kw: Color,
    pub tok_str: Color,
    pub tok_num: Color,
    pub tok_cmt: Color,
    pub shadow_card: Shadow,
    pub shadow_float: Shadow,
}

pub fn palette(dark: bool) -> UiPalette {
    if dark {
        UiPalette {
            dark: true,
            bg_canvas: Color::from_rgb8(0x0b, 0x0e, 0x14),
            bg_sidebar: Color::from_rgb8(0x10, 0x14, 0x1d),
            bg_surface: Color::from_rgb8(0x15, 0x1a, 0x25),
            bg_surface_2: Color::from_rgb8(0x1c, 0x22, 0x31),
            bg_surface_hover: Color::from_rgb8(0x23, 0x2b, 0x3e),
            bg_glass: Color::from_rgba8(0x10, 0x14, 0x1d, 0.94),
            bg_code: Color::from_rgb8(0x0d, 0x11, 0x1a),
            bg_user_bubble: Color::from_rgb8(0x23, 0x2d, 0x4f),
            border_subtle: Color::from_rgba8(0xff, 0xff, 0xff, 0.08),
            border_strong: Color::from_rgba8(0xff, 0xff, 0xff, 0.16),
            ink: Color::from_rgb8(0xee, 0xf2, 0xff),
            ink_secondary: Color::from_rgb8(0xb4, 0xbd, 0xd4),
            ink_muted: Color::from_rgb8(0x7c, 0x87, 0xa3),
            accent: Color::from_rgb8(0x63, 0x66, 0xf1),
            accent_hover: Color::from_rgb8(0x7c, 0x7f, 0xf8),
            accent_soft: Color::from_rgba8(0x63, 0x66, 0xf1, 0.16),
            accent_border: Color::from_rgba8(0x81, 0x8c, 0xf8, 0.42),
            green: Color::from_rgb8(0x10, 0xb9, 0x81),
            green_soft: Color::from_rgba8(0x10, 0xb9, 0x81, 0.14),
            amber: Color::from_rgb8(0xf5, 0x9e, 0x0b),
            amber_soft: Color::from_rgba8(0xf5, 0x9e, 0x0b, 0.14),
            red: Color::from_rgb8(0xf4, 0x3f, 0x5e),
            red_soft: Color::from_rgba8(0xf4, 0x3f, 0x5e, 0.15),
            diff_add_bg: Color::from_rgba8(0x10, 0xb9, 0x81, 0.14),
            diff_del_bg: Color::from_rgba8(0xf4, 0x3f, 0x5e, 0.15),
            tok_kw: Color::from_rgb8(0xc0, 0x84, 0xfc),
            tok_str: Color::from_rgb8(0x34, 0xd3, 0x99),
            tok_num: Color::from_rgb8(0xfb, 0xbf, 0x24),
            tok_cmt: Color::from_rgb8(0x64, 0x74, 0x8b),
            shadow_card: Shadow {
                color: Color::from_rgba(0.0, 0.0, 0.0, 0.26),
                offset: Vector::new(0.0, 2.0),
                blur_radius: 10.0,
            },
            shadow_float: Shadow {
                color: Color::from_rgba(0.0, 0.0, 0.0, 0.52),
                offset: Vector::new(0.0, 10.0),
                blur_radius: 28.0,
            },
        }
    } else {
        UiPalette {
            dark: false,
            bg_canvas: Color::from_rgb8(0xf5, 0xf7, 0xfb),
            bg_sidebar: Color::from_rgb8(0xee, 0xf1, 0xf7),
            bg_surface: Color::from_rgb8(0xff, 0xff, 0xff),
            bg_surface_2: Color::from_rgb8(0xf8, 0xfa, 0xfc),
            bg_surface_hover: Color::from_rgb8(0xed, 0xf2, 0xff),
            bg_glass: Color::from_rgba8(0xff, 0xff, 0xff, 0.94),
            bg_code: Color::from_rgb8(0xf1, 0xf5, 0xf9),
            bg_user_bubble: Color::from_rgb8(0xe0, 0xe7, 0xff),
            border_subtle: Color::from_rgba8(0x0f, 0x17, 0x2a, 0.08),
            border_strong: Color::from_rgba8(0x0f, 0x17, 0x2a, 0.16),
            ink: Color::from_rgb8(0x0f, 0x17, 0x2a),
            ink_secondary: Color::from_rgb8(0x33, 0x41, 0x55),
            ink_muted: Color::from_rgb8(0x64, 0x74, 0x8b),
            accent: Color::from_rgb8(0x4f, 0x46, 0xe5),
            accent_hover: Color::from_rgb8(0x43, 0x38, 0xca),
            accent_soft: Color::from_rgba8(0x4f, 0x46, 0xe5, 0.10),
            accent_border: Color::from_rgba8(0x4f, 0x46, 0xe5, 0.36),
            green: Color::from_rgb8(0x05, 0x96, 0x69),
            green_soft: Color::from_rgba8(0x05, 0x96, 0x69, 0.10),
            amber: Color::from_rgb8(0xd9, 0x77, 0x06),
            amber_soft: Color::from_rgba8(0xd9, 0x77, 0x06, 0.12),
            red: Color::from_rgb8(0xe1, 0x1d, 0x48),
            red_soft: Color::from_rgba8(0xe1, 0x1d, 0x48, 0.10),
            diff_add_bg: Color::from_rgba8(0x10, 0xb9, 0x81, 0.12),
            diff_del_bg: Color::from_rgba8(0xf4, 0x3f, 0x5e, 0.11),
            tok_kw: Color::from_rgb8(0x7e, 0x22, 0xce),
            tok_str: Color::from_rgb8(0x04, 0x78, 0x57),
            tok_num: Color::from_rgb8(0xb4, 0x53, 0x09),
            tok_cmt: Color::from_rgb8(0x64, 0x74, 0x8b),
            shadow_card: Shadow {
                color: Color::from_rgba(0.06, 0.09, 0.16, 0.06),
                offset: Vector::new(0.0, 2.0),
                blur_radius: 10.0,
            },
            shadow_float: Shadow {
                color: Color::from_rgba(0.06, 0.09, 0.16, 0.15),
                offset: Vector::new(0.0, 10.0),
                blur_radius: 26.0,
            },
        }
    }
}

pub fn iced_theme(dark: bool) -> Theme {
    let p = palette(dark);
    Theme::custom(
        if dark { "QwenDark" } else { "QwenLight" },
        iced_palette::Palette {
            background: p.bg_canvas,
            text: p.ink,
            primary: p.accent,
            success: p.green,
            warning: p.amber,
            danger: p.red,
        },
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PillKind {
    Neutral,
    Ok,
    Accent,
    Warn,
}

pub fn box_style(
    bg: Color,
    border_color: Color,
    radius: f32,
    border_width: f32,
    text_color: Option<Color>,
    shadow: Shadow,
) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(bg)),
        text_color,
        border: Border {
            color: border_color,
            width: border_width,
            radius: radius.into(),
        },
        shadow,
        snap: true,
    }
}

pub fn canvas_style(pal: UiPalette) -> impl Fn(&Theme) -> container::Style {
    box_style(pal.bg_canvas, Color::TRANSPARENT, 0.0, 0.0, Some(pal.ink), Shadow::default())
}

pub fn sidebar_style(pal: UiPalette) -> impl Fn(&Theme) -> container::Style {
    box_style(pal.bg_sidebar, pal.border_subtle, 0.0, 1.0, Some(pal.ink), Shadow::default())
}

pub fn card_style(pal: UiPalette) -> impl Fn(&Theme) -> container::Style {
    box_style(pal.bg_surface, pal.border_subtle, 14.0, 1.0, Some(pal.ink), pal.shadow_card)
}

pub fn card_flat_style(pal: UiPalette) -> impl Fn(&Theme) -> container::Style {
    box_style(pal.bg_surface, pal.border_subtle, 14.0, 1.0, Some(pal.ink), Shadow::default())
}

pub fn surface2_style(pal: UiPalette, radius: f32) -> impl Fn(&Theme) -> container::Style {
    box_style(pal.bg_surface_2, pal.border_subtle, radius, 1.0, Some(pal.ink), Shadow::default())
}

pub fn topbar_style(pal: UiPalette) -> impl Fn(&Theme) -> container::Style {
    box_style(pal.bg_glass, Color::TRANSPARENT, 0.0, 0.0, Some(pal.ink), Shadow::default())
}

pub fn status_strip_style(pal: UiPalette) -> impl Fn(&Theme) -> container::Style {
    box_style(pal.bg_surface_2, Color::TRANSPARENT, 0.0, 0.0, Some(pal.ink), Shadow::default())
}

pub fn pill_container_style(pal: UiPalette, kind: PillKind) -> impl Fn(&Theme) -> container::Style {
    let (bg, border, txt) = match kind {
        PillKind::Neutral => (pal.bg_surface_2, pal.border_subtle, pal.ink_secondary),
        PillKind::Ok => (pal.green_soft, pal.green.scale_alpha(0.35), pal.green),
        PillKind::Accent => (pal.accent_soft, pal.accent_border, pal.accent),
        PillKind::Warn => (pal.amber_soft, pal.amber.scale_alpha(0.40), pal.amber),
    };
    box_style(bg, border, 99.0, 1.0, Some(txt), Shadow::default())
}

pub fn user_bubble_style(pal: UiPalette) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(pal.bg_user_bubble)),
        text_color: Some(pal.ink),
        border: Border {
            color: pal.accent_border,
            width: 1.0,
            radius: border::Radius {
                top_left: 18.0,
                top_right: 18.0,
                bottom_right: 4.0,
                bottom_left: 18.0,
            },
        },
        shadow: pal.shadow_card,
        snap: true,
    }
}

pub fn blockquote_style(pal: UiPalette) -> impl Fn(&Theme) -> container::Style {
    box_style(pal.accent_soft, pal.accent_border, 8.0, 1.0, Some(pal.ink_secondary), Shadow::default())
}

pub fn composer_box_style(pal: UiPalette, focused: bool) -> impl Fn(&Theme) -> container::Style {
    box_style(
        pal.bg_surface,
        if focused { pal.accent } else { pal.border_strong },
        18.0,
        if focused { 1.5 } else { 1.0 },
        Some(pal.ink),
        pal.shadow_card,
    )
}

pub fn rule_subtle(pal: UiPalette) -> impl Fn(&Theme) -> rule::Style {
    move |_| rule::Style {
        color: pal.border_subtle,
        radius: 0.0.into(),
        fill_mode: rule::FillMode::Full,
        snap: true,
    }
}

pub fn btn_primary(pal: UiPalette) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let bg = match status {
            button::Status::Hovered | button::Status::Pressed => pal.accent_hover,
            button::Status::Disabled => pal.accent.scale_alpha(0.45),
            button::Status::Active => pal.accent,
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: Color::WHITE,
            border: border::rounded(10),
            ..button::Style::default()
        }
    }
}

pub fn btn_secondary(pal: UiPalette) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let (bg, txt) = match status {
            button::Status::Hovered | button::Status::Pressed => (pal.bg_surface_hover, pal.ink),
            button::Status::Disabled => (pal.bg_surface_2.scale_alpha(0.6), pal.ink_muted),
            button::Status::Active => (pal.bg_surface_2, pal.ink),
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: txt,
            border: Border {
                color: pal.border_strong,
                width: 1.0,
                radius: 10.0.into(),
            },
            ..button::Style::default()
        }
    }
}

pub fn btn_ghost(pal: UiPalette) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let (bg, txt) = match status {
            button::Status::Hovered | button::Status::Pressed => (Some(pal.bg_surface_hover), pal.ink),
            button::Status::Disabled => (None, pal.ink_muted),
            button::Status::Active => (None, pal.ink_secondary),
        };
        button::Style {
            background: bg.map(Background::Color),
            text_color: txt,
            border: border::rounded(8),
            ..button::Style::default()
        }
    }
}

pub fn btn_danger(pal: UiPalette) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let (bg, txt) = match status {
            button::Status::Hovered | button::Status::Pressed => (pal.red, Color::WHITE),
            button::Status::Disabled => (pal.red_soft, pal.red.scale_alpha(0.45)),
            button::Status::Active => (pal.red_soft, pal.red),
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: txt,
            border: Border {
                color: pal.red.scale_alpha(0.35),
                width: 1.0,
                radius: 9.0.into(),
            },
            ..button::Style::default()
        }
    }
}

pub fn btn_icon(pal: UiPalette) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let (bg, txt) = match status {
            button::Status::Hovered | button::Status::Pressed => (pal.bg_surface_hover, pal.ink),
            button::Status::Disabled => (pal.bg_surface, pal.ink_muted),
            button::Status::Active => (pal.bg_surface, pal.ink_secondary),
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: txt,
            border: Border {
                color: pal.border_subtle,
                width: 1.0,
                radius: 9.0.into(),
            },
            ..button::Style::default()
        }
    }
}

pub fn btn_pill_select(pal: UiPalette, active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let (bg, border, txt) = if active {
            (pal.accent_soft, pal.accent_border, pal.accent)
        } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
            (pal.bg_surface, pal.border_subtle, pal.ink)
        } else {
            (Color::TRANSPARENT, Color::TRANSPARENT, pal.ink_muted)
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: txt,
            border: Border {
                color: border,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..button::Style::default()
        }
    }
}

pub fn btn_tab(pal: UiPalette, active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let (bg, txt) = if active {
            (pal.accent, Color::WHITE)
        } else if matches!(status, button::Status::Disabled) {
            (Color::TRANSPARENT, pal.ink_muted.scale_alpha(0.45))
        } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
            (pal.bg_surface_hover, pal.ink)
        } else {
            (Color::TRANSPARENT, pal.ink_secondary)
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: txt,
            border: border::rounded(8),
            ..button::Style::default()
        }
    }
}

pub fn btn_card(pal: UiPalette, selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let disabled = matches!(status, button::Status::Disabled);
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let (bg, border, txt) = if selected {
            (pal.accent_soft, pal.accent_border, pal.ink)
        } else if disabled {
            (
                pal.bg_surface_2.scale_alpha(0.55),
                pal.border_subtle.scale_alpha(0.65),
                pal.ink_muted.scale_alpha(0.55),
            )
        } else if hovered {
            (pal.bg_surface_hover, pal.border_strong, pal.ink)
        } else {
            (pal.bg_surface, pal.border_subtle, pal.ink)
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: txt,
            border: Border {
                color: border,
                width: 1.0,
                radius: 12.0.into(),
            },
            ..button::Style::default()
        }
    }
}

pub fn btn_history_row(pal: UiPalette, selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let disabled = matches!(status, button::Status::Disabled);
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let (bg, border, txt) = if selected {
            (pal.accent_soft, pal.accent_border, pal.ink)
        } else if disabled {
            (Color::TRANSPARENT, Color::TRANSPARENT, pal.ink_muted.scale_alpha(0.45))
        } else if hovered {
            (pal.bg_surface_hover, Color::TRANSPARENT, pal.ink)
        } else {
            (Color::TRANSPARENT, Color::TRANSPARENT, pal.ink)
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: txt,
            border: Border {
                color: border,
                width: 1.0,
                radius: 10.0.into(),
            },
            ..button::Style::default()
        }
    }
}

pub fn btn_chip(pal: UiPalette, muted: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let disabled = matches!(status, button::Status::Disabled);
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let (bg, border, txt) = if disabled {
            (
                pal.bg_surface_2.scale_alpha(0.55),
                pal.border_subtle.scale_alpha(0.70),
                pal.ink_muted.scale_alpha(0.50),
            )
        } else if muted {
            (
                if hovered { pal.bg_surface_hover } else { pal.bg_surface_2 },
                pal.border_subtle,
                pal.ink_secondary,
            )
        } else {
            (
                if hovered { pal.accent.scale_alpha(0.24) } else { pal.accent_soft },
                pal.accent_border,
                pal.accent,
            )
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: txt,
            border: Border {
                color: border,
                width: 1.0,
                radius: 9.0.into(),
            },
            ..button::Style::default()
        }
    }
}

pub fn input_style(pal: UiPalette) -> impl Fn(&Theme, text_input::Status) -> text_input::Style {
    move |_, status| {
        let border_c = match status {
            text_input::Status::Focused { .. } => pal.accent,
            text_input::Status::Hovered => pal.border_strong,
            _ => pal.border_subtle,
        };
        text_input::Style {
            background: Background::Color(pal.bg_surface),
            border: Border {
                color: border_c,
                width: 1.0,
                radius: 9.0.into(),
            },
            icon: pal.ink_muted,
            placeholder: pal.ink_muted,
            value: pal.ink,
            selection: pal.accent_soft,
        }
    }
}

pub fn editor_transparent_style(pal: UiPalette) -> impl Fn(&Theme, text_editor::Status) -> text_editor::Style {
    move |_, status| text_editor::Style {
        background: Background::Color(Color::TRANSPARENT),
        border: Border::default(),
        placeholder: pal.ink_muted,
        value: if matches!(status, text_editor::Status::Disabled) {
            pal.ink_muted
        } else {
            pal.ink
        },
        selection: pal.accent_soft,
    }
}

pub fn pick_list_style(pal: UiPalette) -> impl Fn(&Theme, pick_list::Status) -> pick_list::Style {
    move |_, status| {
        let border_c = match status {
            pick_list::Status::Opened { .. } | pick_list::Status::Hovered => pal.accent_border,
            pick_list::Status::Active => pal.border_subtle,
        };
        pick_list::Style {
            text_color: pal.ink,
            placeholder_color: pal.ink_muted,
            handle_color: pal.ink_secondary,
            background: Background::Color(pal.bg_surface_2),
            border: Border {
                color: border_c,
                width: 1.0,
                radius: 9.0.into(),
            },
        }
    }
}

pub fn pick_menu_style(pal: UiPalette) -> impl Fn(&Theme) -> menu::Style {
    move |_| menu::Style {
        background: Background::Color(pal.bg_surface),
        border: Border {
            color: pal.border_strong,
            width: 1.0,
            radius: 10.0.into(),
        },
        text_color: pal.ink,
        selected_text_color: Color::WHITE,
        selected_background: Background::Color(pal.accent),
        shadow: pal.shadow_float,
    }
}

pub fn scrollable_style(pal: UiPalette) -> impl Fn(&Theme, scrollable::Status) -> scrollable::Style {
    move |t, status| {
        let mut s = scrollable::default(t, status);
        s.container = container::Style::default();
        s.vertical_rail.background = Some(Background::Color(Color::TRANSPARENT));
        s.vertical_rail.border = Border::default();
        s.vertical_rail.scroller.background = Background::Color(match status {
            scrollable::Status::Hovered { is_vertical_scrollbar_hovered: true, .. }
            | scrollable::Status::Dragged { is_vertical_scrollbar_dragged: true, .. } => pal.ink_muted,
            _ => pal.border_strong,
        });
        s.vertical_rail.scroller.border = border::rounded(99);
        s.horizontal_rail.background = Some(Background::Color(Color::TRANSPARENT));
        s.horizontal_rail.border = Border::default();
        s.horizontal_rail.scroller.background = Background::Color(pal.border_strong);
        s.horizontal_rail.scroller.border = border::rounded(99);
        s
    }
}

pub fn slider_style(pal: UiPalette) -> impl Fn(&Theme, slider::Status) -> slider::Style {
    move |_, status| {
        let active_c = match status {
            slider::Status::Hovered | slider::Status::Dragged => pal.accent_hover,
            slider::Status::Active => pal.accent,
        };
        slider::Style {
            rail: slider::Rail {
                backgrounds: (
                    Background::Color(active_c),
                    Background::Color(pal.bg_canvas),
                ),
                width: 5.0,
                border: Border {
                    radius: 99.0.into(),
                    width: 1.0,
                    color: pal.border_subtle,
                },
            },
            handle: slider::Handle {
                shape: slider::HandleShape::Circle { radius: 7.0 },
                background: Background::Color(active_c),
                border_width: 1.5,
                border_color: Color::WHITE,
            },
        }
    }
}

pub fn toggler_style(pal: UiPalette) -> impl Fn(&Theme, toggler::Status) -> toggler::Style {
    move |_, status| {
        let (toggled, disabled) = match status {
            toggler::Status::Active { is_toggled } | toggler::Status::Hovered { is_toggled } => {
                (is_toggled, false)
            }
            toggler::Status::Disabled { is_toggled } => (is_toggled, true),
        };
        toggler::Style {
            background: Background::Color(if toggled {
                if disabled { pal.accent.scale_alpha(0.45) } else { pal.accent }
            } else {
                pal.bg_canvas
            }),
            background_border_width: 1.0,
            background_border_color: if toggled { pal.accent } else { pal.border_strong },
            foreground: Background::Color(if disabled { pal.ink_muted } else { Color::WHITE }),
            foreground_border_width: 0.0,
            foreground_border_color: Color::TRANSPARENT,
            text_color: Some(if disabled { pal.ink_muted } else { pal.ink }),
            border_radius: None,
            padding_ratio: 0.14,
        }
    }
}
