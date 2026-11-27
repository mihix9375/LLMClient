use crate::ui::app::{ModalKind, PaletteAction, UiState};
use crate::ui::icons::{icon, icon_button, IconKind};
use crate::ui::theme::{self, MONO_FONT, UI_FONT_BOLD, UiPalette};
use crate::ui::Message;
use iced::widget::{
    button, column, container, image as img_w, mouse_area, opaque, row, scrollable, space, text,
    text_input,
};
use iced::{Alignment, Color, ContentFit, Element, Length, Padding};

pub fn palette_items(query: &str) -> Vec<PaletteAction> {
    let all = vec![
        PaletteAction { title: "新しいチャットを開始", hint: "Ctrl+N", kind: "new_chat", disabled: false },
        PaletteAction { title: "サーバーへ接続 / 再接続", hint: "F5", kind: "connect", disabled: false },
        PaletteAction { title: "テーマ切り替え (ダーク / ライト)", hint: "", kind: "theme", disabled: false },
        PaletteAction { title: "動作モード: 標準", hint: "", kind: "mode:standard", disabled: false },
        PaletteAction { title: "動作モード: チャット専用", hint: "", kind: "mode:chat", disabled: false },
        PaletteAction { title: "動作モード: コーディング用", hint: "", kind: "mode:coding", disabled: false },
        PaletteAction { title: "動作モード: ゲーム・PC操作", hint: "", kind: "mode:pc_control", disabled: false },
        PaletteAction { title: "会話を圧縮 (/compact)", hint: "", kind: "slash:/compact", disabled: false },
        PaletteAction { title: "学習メモを表示 (/learn list)", hint: "", kind: "slash:/learn list", disabled: false },
        PaletteAction { title: "スケジュール一覧 (/schedule list)", hint: "", kind: "slash:/schedule list", disabled: false },
        PaletteAction { title: "スラッシュコマンド一覧 (/help)", hint: "", kind: "slash:/help", disabled: false },
        PaletteAction { title: "モデルを再起動 (/models restart)", hint: "", kind: "restart_server", disabled: false },
        PaletteAction { title: "8B 担当パネルを開く", hint: "利用不可", kind: "pane:workers", disabled: true },
        PaletteAction { title: "作業環境・PC操作パネルを開く", hint: "", kind: "pane:tools", disabled: false },
        PaletteAction { title: "設定・チューニングを開く", hint: "", kind: "pane:settings", disabled: false },
        PaletteAction { title: "PC操作の緊急停止", hint: "Ctrl+Shift+F12", kind: "stop_pc", disabled: false },
    ];
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        all
    } else {
        all.into_iter()
            .filter(|a| a.title.to_ascii_lowercase().contains(&q))
            .collect()
    }
}

pub fn render_overlays<'a>(state: &'a UiState, pal: UiPalette) -> Vec<Element<'a, Message>> {
    let mut layers: Vec<Element<'a, Message>> = Vec::new();

    if state.anim.drop_overlay.is_visible() {
        let v = state.anim.drop_overlay.value();
        let box_card = container(
            column![
                icon(IconKind::Paperclip, 38.0, pal.accent),
                text("ファイルをドロップして添付").size(18.0).font(UI_FONT_BOLD).color(pal.ink),
            ]
            .spacing(8)
            .align_x(Alignment::Center),
        )
        .padding([40, 56])
        .style(theme::box_style(
            pal.bg_surface.scale_alpha(v),
            pal.accent.scale_alpha(v),
            20.0,
            2.0,
            Some(pal.ink),
            pal.shadow_float,
        ));

        layers.push(
            container(box_card)
                .center(Length::Fill)
                .style(theme::box_style(
                    Color::from_rgba(0.04, 0.05, 0.08, 0.72 * v),
                    Color::TRANSPARENT,
                    0.0,
                    0.0,
                    None,
                    Default::default(),
                ))
                .into(),
        );
    }

    if state.anim.modal.is_visible() {
        if let Some(ref kind) = state.displayed_modal {
            let v = state.anim.modal.value();
            let card: Element<'a, Message> = match kind {
                ModalKind::Palette => render_palette_card(state, pal, v),
                ModalKind::Shortcuts => render_shortcuts_card(pal, v),
                ModalKind::Lightbox { title, img, text: txt } => {
                    render_lightbox_card(state, title, img.as_ref(), txt, pal, v)
                }
            };
            let backdrop = mouse_area(
                container(mouse_area(opaque(card)).on_press(Message::Noop))
                    .center(Length::Fill)
                    .padding(24)
                    .style(theme::box_style(
                        Color::from_rgba(0.0, 0.0, 0.0, 0.60 * v),
                        Color::TRANSPARENT,
                        0.0,
                        0.0,
                        None,
                        Default::default(),
                    )),
            )
            .on_press(Message::CloseModal);
            layers.push(backdrop.into());
        }
    }

    if !state.anim.toasts.is_empty() {
        let mut toast_col = column![].spacing(8).width(Length::Fixed(320.0)).height(Length::Shrink);
        for t in &state.anim.toasts {
            let (alpha, dx) = t.opacity_and_offset();
            if alpha <= 0.01 {
                continue;
            }
            let bar_c = match t.level.as_str() {
                "ok" => pal.green,
                "warn" => pal.amber,
                "error" => pal.red,
                _ => pal.accent,
            };
            let card = container(
                row![
                    container(space().width(4).height(Length::Fixed(18.0)))
                        .style(theme::box_style(
                            bar_c.scale_alpha(alpha),
                            Color::TRANSPARENT,
                            2.0,
                            0.0,
                            None,
                            Default::default(),
                        )),
                    text(t.message.clone())
                        .size(12.5)
                        .color(pal.ink.scale_alpha(alpha))
                        .width(Length::Fill),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            )
            .padding([9, 13])
            .width(Length::Fill)
            .height(Length::Shrink)
            .style(theme::box_style(
                pal.bg_surface.scale_alpha(alpha),
                pal.border_strong.scale_alpha(alpha),
                12.0,
                1.0,
                Some(pal.ink.scale_alpha(alpha)),
                pal.shadow_float,
            ));

            toast_col = toast_col.push(
                container(card)
                    .width(Length::Fill)
                    .height(Length::Shrink)
                    .padding(Padding {
                        top: 0.0,
                        right: 0.0,
                        bottom: 0.0,
                        left: dx.max(0.0),
                    }),
            );
        }
        layers.push(
            container(toast_col)
                .align_right(Length::Fill)
                .align_bottom(Length::Fill)
                .padding(20)
                .into(),
        );
    }

    layers
}

fn render_palette_card<'a>(state: &'a UiState, pal: UiPalette, v: f32) -> Element<'a, Message> {
    let items = palette_items(&state.palette_query);
    let sel_idx = state.palette_idx.min(items.len().saturating_sub(1));

    let inp = row![
        icon(IconKind::Command, 16.0, pal.accent),
        text_input("コマンドを検索…", &state.palette_query)
            .id("palette-input")
            .on_input(Message::PaletteQueryChanged)
            .on_submit(Message::PaletteRun(sel_idx))
            .size(14.5)
            .padding([8, 10])
            .style(theme::input_style(pal)),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .padding([12, 14]);

    let list_col = if items.is_empty() {
        column![
            container(
                text("該当なし")
                    .size(13.0)
                    .color(pal.ink_muted)
            )
            .padding(18)
        ]
    } else {
        column(items.into_iter().enumerate().map(|(i, item)| {
            let selected = !item.disabled && i == sel_idx;
            let title_c = if item.disabled {
                pal.ink_muted.scale_alpha(0.50)
            } else if selected {
                pal.accent
            } else {
                pal.ink
            };
            let hint_c = if item.disabled {
                pal.ink_muted.scale_alpha(0.55)
            } else {
                pal.ink_muted
            };
            button(
                row![
                    text(item.title).size(13.5).color(title_c),
                    space::horizontal(),
                    if item.hint.is_empty() {
                        Element::from(space())
                    } else {
                        container(text(item.hint).size(11.0).font(MONO_FONT).color(hint_c))
                            .padding([2, 6])
                            .style(theme::surface2_style(pal, 6.0))
                            .into()
                    }
                ]
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([9, 12])
            .style(theme::btn_history_row(pal, selected))
            .on_press_maybe((!item.disabled).then_some(Message::PaletteRun(i)))
            .into()
        }))
        .spacing(4)
        .padding(8)
    };

    container(column![
        inp,
        scrollable(list_col)
            .height(Length::Fixed(320.0))
            .style(theme::scrollable_style(pal)),
    ])
    .width(Length::Fixed(540.0))
    .style(theme::box_style(
        pal.bg_surface.scale_alpha(0.95 + 0.05 * v),
        pal.border_strong.scale_alpha(v),
        18.0,
        1.0,
        Some(pal.ink),
        pal.shadow_float,
    ))
    .clip(true)
    .into()
}

fn render_shortcuts_card<'a>(pal: UiPalette, v: f32) -> Element<'a, Message> {
    let rows = [
        ("送信 / 追加指示 / 回答", "Ctrl + Enter"),
        ("新しいチャット", "Ctrl + N"),
        ("コマンドパレット", "Ctrl + K"),
        ("会話内を検索", "Ctrl + F"),
        ("サイドバー表示切替", "Ctrl + B"),
        ("クリップボードから画像・ファイル貼付", "Ctrl + V"),
        ("画面・ドロワーを閉じる / 検索クリア", "Esc"),
        ("画面再読み込み / 再接続", "F5"),
        ("PC操作の緊急停止（グローバル）", "Ctrl + Shift + F12"),
    ];
    let head = row![
        text("キーボードショートカット").size(15.0).font(UI_FONT_BOLD).wrapping(iced::widget::text::Wrapping::None),
        space::horizontal(),
        icon_button(IconKind::Close, 28.0, 28.0, 13.0, pal, Message::CloseModal),
    ]
    .align_y(Alignment::Center)
    .padding([12, 16]);

    let body = column(rows.into_iter().map(|(label, kbd)| {
        row![
            text(label).size(13.0).color(pal.ink),
            space::horizontal(),
            container(text(kbd).size(12.0).font(MONO_FONT).color(pal.ink_secondary))
                .padding([3, 8])
                .style(theme::surface2_style(pal, 6.0)),
        ]
        .align_y(Alignment::Center)
        .into()
    }))
    .spacing(10)
    .padding([12, 18]);

    container(column![head, body])
        .width(Length::Fixed(520.0))
        .style(theme::box_style(
            pal.bg_surface.scale_alpha(0.95 + 0.05 * v),
            pal.border_strong.scale_alpha(v),
            18.0,
            1.0,
            Some(pal.ink),
            pal.shadow_float,
        ))
        .clip(true)
        .into()
}

fn render_lightbox_card<'a>(
    state: &'a UiState,
    title: &'a str,
    img: Option<&'a img_w::Handle>,
    txt: &'a str,
    pal: UiPalette,
    v: f32,
) -> Element<'a, Message> {
    let mut head_items: Vec<Element<'a, Message>> = vec![
        container(
            text(title.to_string())
                .size(14.5)
                .font(UI_FONT_BOLD)
                .wrapping(iced::widget::text::Wrapping::None),
        )
        .width(Length::Fill)
        .clip(true)
        .into(),
    ];
    if img.is_none() && !state.loading_preview {
        let copied = state.copied_key.as_deref() == Some("lightbox");
        head_items.push(
            button(text(if copied { "コピー済 ✓" } else { "内容をコピー" }).size(12.0))
                .padding([5, 10])
                .style(theme::btn_ghost(pal))
                .on_press(Message::CopyText(txt.to_string(), Some("lightbox".into())))
                .into(),
        );
    }
    head_items.push(icon_button(IconKind::Close, 28.0, 28.0, 13.0, pal, Message::CloseModal));
    let head = row(head_items)
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([12, 16]);

    let body: Element<'a, Message> = if state.loading_preview {
        container(
            crate::ui::anim::shimmer_skeleton(
                state.anim.shimmer_phase,
                vec![(0.85, 18.0), (0.95, 14.0), (0.72, 14.0), (0.90, 14.0)],
                10.0,
                8.0,
                pal,
            ),
        )
        .padding(20)
        .into()
    } else if let Some(handle) = img {
        container(
            img_w(handle.clone())
                .width(Length::Fill)
                .height(Length::Fixed(520.0))
                .content_fit(ContentFit::Contain),
        )
        .padding(16)
        .into()
    } else if txt.is_empty() {
        container(text("プレビューできる内容がありません").size(13.0).color(pal.ink_muted))
            .padding(20)
            .into()
    } else {
        scrollable(
            container(text(txt.to_string()).size(12.5).font(MONO_FONT).color(pal.ink))
                .padding(14)
                .width(Length::Fill)
                .style(theme::surface2_style(pal, 10.0)),
        )
        .height(Length::Fixed(520.0))
        .style(theme::scrollable_style(pal))
        .into()
    };

    container(column![head, container(body).padding(Padding { top: 0.0, right: 16.0, bottom: 16.0, left: 16.0 })])
        .width(Length::Fixed(860.0))
        .style(theme::box_style(
            pal.bg_surface.scale_alpha(0.95 + 0.05 * v),
            pal.border_strong.scale_alpha(v),
            18.0,
            1.0,
            Some(pal.ink),
            pal.shadow_float,
        ))
        .clip(true)
        .into()
}
