use crate::ui::anim::{meter_bar, pulse_dot, shimmer_skeleton};
use crate::ui::app::{
    DrawerPane, EffortOpt, HistoryFilter, ModalKind, ShareModeOpt, TuneModeOpt, UiState,
};
use crate::ui::icons::{icon, icon_button, IconKind};
use crate::ui::theme::{self, MONO_FONT, UI_FONT_BOLD, UiPalette};
use crate::ui::Message;
use chrono::{DateTime, Local, Utc};
use iced::widget::text::Wrapping;
use iced::widget::{
    button, column, container, pick_list, row, rule, scrollable, slider, space, text, text_input,
    toggler,
};
use iced::{alignment, Alignment, Element, Length};
use serde_json::Value;

fn date_group_label(iso: &str) -> &'static str {
    let Ok(dt) = DateTime::parse_from_rfc3339(iso) else {
        return "以前";
    };
    let local_day = dt.with_timezone(&Local).date_naive();
    let today = Utc::now().with_timezone(&Local).date_naive();
    let diff = (today - local_day).num_days();
    if diff <= 0 {
        "今日"
    } else if diff == 1 {
        "昨日"
    } else if diff <= 7 {
        "過去7日"
    } else {
        "以前"
    }
}

pub fn render_sidebar<'a>(state: &'a UiState, pal: UiPalette) -> Element<'a, Message> {
    let v = state.anim.sidebar.value();
    if !state.anim.sidebar.is_visible() {
        return space().width(0).into();
    }
    let width = 272.0 * v;

    let top_row = row![
        container(
            text("Q")
                .size(15.0)
                .font(UI_FONT_BOLD)
                .line_height(1.0)
                .color(iced::Color::WHITE)
                .align_x(alignment::Horizontal::Center)
                .align_y(alignment::Vertical::Center),
        )
        .width(30)
        .height(30)
        .center(30)
        .style(theme::box_style(
            pal.accent,
            iced::Color::TRANSPARENT,
            9.0,
            0.0,
            Some(iced::Color::WHITE),
            Default::default(),
        )),
        text("Qwen27").size(15.0).font(UI_FONT_BOLD).color(pal.ink).wrapping(Wrapping::None),
        space::horizontal(),
        icon_button(
            if state.is_dark() { IconKind::Sun } else { IconKind::Moon },
            30.0,
            30.0,
            15.0,
            pal,
            Message::ToggleTheme,
        ),
        icon_button(
            IconKind::Help,
            30.0,
            30.0,
            14.0,
            pal,
            Message::OpenModal(ModalKind::Shortcuts),
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let new_chat = button(
        row![
            icon(IconKind::Plus, 13.0, iced::Color::WHITE),
            text("新しいチャット")
                .size(13.0)
                .font(UI_FONT_BOLD)
                .wrapping(Wrapping::None),
            space::horizontal(),
            container(text("Ctrl+N").size(10.5).font(MONO_FONT).wrapping(Wrapping::None))
                .padding([2, 6])
                .style(theme::box_style(
                    iced::Color::from_rgba(0.0, 0.0, 0.0, 0.2),
                    iced::Color::from_rgba(1.0, 1.0, 1.0, 0.2),
                    5.0,
                    1.0,
                    Some(iced::Color::WHITE),
                    Default::default(),
                )),
        ]
        .spacing(7)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([9, 12])
    .style(theme::btn_primary(pal))
    .on_press(Message::NewChat);

    let hist_head = row![
        text("最近のチャット").size(11.5).font(UI_FONT_BOLD).color(pal.ink_muted).wrapping(Wrapping::None),
        space::horizontal(),
        icon_button(IconKind::Refresh, 26.0, 26.0, 13.0, pal, Message::RefreshHistory),
    ]
    .align_y(Alignment::Center);

    let search_box = text_input("履歴を検索…", &state.history_query)
        .on_input(Message::HistorySearchChanged)
        .size(12.5)
        .padding([7, 10])
        .style(theme::input_style(pal));

    let filters = row![
        filter_pill("すべて", HistoryFilter::All, state.history_filter, pal),
        filter_pill("進行中", HistoryFilter::Active, state.history_filter, pal),
        filter_pill("完了", HistoryFilter::Done, state.history_filter, pal),
    ]
    .spacing(4);

    let list_area: Element<'a, Message> = if (state.connecting || state.loading_history)
        && state.conversations.is_empty()
    {
        container(shimmer_skeleton(
            state.anim.shimmer_phase,
            vec![
                (0.95, 42.0),
                (0.88, 42.0),
                (0.92, 42.0),
                (0.80, 42.0),
                (0.90, 42.0),
            ],
            8.0,
            10.0,
            pal,
        ))
        .padding([6, 4])
        .height(Length::Fill)
        .into()
    } else {
        build_history_list(state, pal)
    };

    let workers_btn = button(
        row![
            pulse_dot(state.anim.pulse_phase, pal.ink_muted.scale_alpha(0.40), false),
            text("8B 担当パネル")
                .size(12.5)
                .font(UI_FONT_BOLD)
                .color(pal.ink_muted.scale_alpha(0.55))
                .wrapping(Wrapping::None),
            space::horizontal(),
            container(
                text("利用不可")
                    .size(10.5)
                    .color(pal.ink_muted.scale_alpha(0.65))
                    .wrapping(Wrapping::None),
            )
            .padding([2, 6])
            .style(theme::box_style(
                pal.bg_surface_2.scale_alpha(0.75),
                pal.border_subtle.scale_alpha(0.75),
                99.0,
                1.0,
                None,
                Default::default(),
            )),
        ]
        .spacing(7)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([7, 10])
    .style(theme::btn_card(pal, false));

    let (conn_c, conn_txt, conn_anim) = if state.connecting {
        (pal.amber, "接続中…", true)
    } else if state.standby {
        (pal.amber, "PC連携待機中", true)
    } else if state.connected {
        (pal.green, "サーバー接続中", false)
    } else {
        (pal.ink_muted, "未接続", false)
    };

    let conn_card = container(
        column![
            row![
                pulse_dot(state.anim.pulse_phase, conn_c, conn_anim),
                text(conn_txt).size(12.0).font(UI_FONT_BOLD).color(conn_c),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
            row![
                button(container(text("接続 / 更新").size(12.5).wrapping(Wrapping::None)).center_x(Length::Fill))
                    .width(Length::Fill)
                    .padding([6, 10])
                    .style(theme::btn_secondary(pal))
                    .on_press(Message::ConnectNow),
                icon_button(IconKind::Settings, 32.0, 30.0, 15.0, pal, Message::TogglePane(DrawerPane::Settings)),
            ]
            .spacing(6),
        ]
        .spacing(8),
    )
    .padding(10)
    .width(Length::Fill)
    .style(theme::card_flat_style(pal));

    let inner = container(
        column![
            top_row,
            new_chat,
            hist_head,
            search_box,
            filters,
            list_area,
            rule::horizontal(1).style(theme::rule_subtle(pal)),
            workers_btn,
            conn_card,
        ]
        .spacing(9),
    )
    .width(Length::Fixed(272.0))
    .height(Length::Fill)
    .padding(12)
    .style(theme::sidebar_style(pal));

    container(inner)
        .width(Length::Fixed(width))
        .height(Length::Fill)
        .clip(true)
        .into()
}

fn filter_pill<'a>(
    label: &'static str,
    target: HistoryFilter,
    cur: HistoryFilter,
    pal: UiPalette,
) -> Element<'a, Message> {
    button(container(text(label).size(11.5).wrapping(Wrapping::None)).center_x(Length::Fill))
        .width(Length::FillPortion(1))
        .padding([5, 6])
        .style(theme::btn_pill_select(pal, target == cur))
        .on_press(Message::SetHistoryFilter(target))
        .into()
}

fn build_history_list<'a>(state: &'a UiState, pal: UiPalette) -> Element<'a, Message> {
    let q = state.history_query.trim().to_ascii_lowercase();
    let active_id = state
        .conversation
        .as_ref()
        .and_then(|c| c.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let filtered: Vec<&Value> = state
        .conversations
        .iter()
        .rev()
        .filter(|c| {
            let st = c.get("status").and_then(|v| v.as_str()).unwrap_or("");
            let is_run = matches!(st, "running" | "queued" | "waiting_user");
            match state.history_filter {
                HistoryFilter::Active if !is_run => return false,
                HistoryFilter::Done if is_run => return false,
                _ => {}
            }
            if !q.is_empty() {
                let title = c.get("title").and_then(|v| v.as_str()).unwrap_or("");
                let cid = c.get("id").and_then(|v| v.as_str()).unwrap_or("");
                if !title.to_ascii_lowercase().contains(&q) && !cid.to_ascii_lowercase().contains(&q) {
                    return false;
                }
            }
            true
        })
        .collect();

    if filtered.is_empty() {
        return container(
            text("該当する会話はありません")
                .size(12.0)
                .color(pal.ink_muted),
        )
        .padding(16)
        .center_x(Length::Fill)
        .height(Length::Fill)
        .into();
    }

    let order = ["今日", "昨日", "過去7日", "以前"];
    let mut col = column![].spacing(4);
    for grp in order {
        let rows: Vec<&&Value> = filtered
            .iter()
            .filter(|c| {
                let ts = c
                    .get("updatedAt")
                    .or_else(|| c.get("createdAt"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                date_group_label(ts) == grp
            })
            .collect();
        if rows.is_empty() {
            continue;
        }
        col = col.push(
            container(text(grp).size(11.0).font(UI_FONT_BOLD).color(pal.ink_muted))
                .padding([4, 6]),
        );
        for c in rows {
            let cid = c.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let title = c
                .get("title")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("(無題のチャット)")
                .to_string();
            let st = c.get("status").and_then(|v| v.as_str()).unwrap_or("ready");
            let jcnt = c.get("jobCount").and_then(|v| v.as_u64()).unwrap_or(0);
            let is_sel = cid == active_id;
            let (dot_c, dot_anim) = match st {
                "running" | "queued" | "waiting_user" => (pal.amber, true),
                "completed" => (pal.green, false),
                "failed" => (pal.red, false),
                _ => (pal.ink_muted, false),
            };
            let meta = if jcnt > 0 {
                format!("{} · {}手", st, jcnt)
            } else {
                st.to_string()
            };
            let cid_copy = cid.clone();
            let card = button(
                column![
                    row![
                        pulse_dot(state.anim.pulse_phase, dot_c, dot_anim),
                        container(
                            text(title)
                                .size(13.0)
                                .color(pal.ink)
                                .wrapping(Wrapping::None),
                        )
                        .width(Length::Fill)
                        .clip(true),
                        button(text("ID").size(10.0).color(pal.ink_muted).wrapping(Wrapping::None))
                            .padding([1, 5])
                            .style(theme::btn_ghost(pal))
                            .on_press(Message::CopyText(cid_copy, None)),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                    container(text(meta).size(11.0).color(pal.ink_muted).wrapping(Wrapping::None))
                        .padding([0, 20]),
                ]
                .spacing(2),
            )
            .width(Length::Fill)
            .padding([7, 9])
            .style(theme::btn_history_row(pal, is_sel))
            .on_press(Message::OpenConversation(cid));
            col = col.push(card);
        }
    }

    scrollable(col)
        .direction(scrollable::Direction::Vertical(
            scrollable::Scrollbar::new()
                .width(6)
                .scroller_width(6)
                .spacing(6),
        ))
        .height(Length::Fill)
        .style(theme::scrollable_style(pal))
        .into()
}

pub fn render_right_drawer<'a>(state: &'a UiState, pal: UiPalette) -> Element<'a, Message> {
    if !state.anim.drawer.is_visible() {
        return space().width(0).into();
    }
    let Some(pane) = state.displayed_pane else {
        return space().width(0).into();
    };
    let target_w = state.anim.drawer_width.value().max(340.0);
    let anim_w = (target_w * state.anim.drawer.value()).max(0.0);

    let mut head_items: Vec<Element<'a, Message>> = vec![
        container(
            text(pane.title())
                .size(15.0)
                .font(UI_FONT_BOLD)
                .wrapping(Wrapping::None),
        )
        .width(Length::Fill)
        .clip(true)
        .into(),
    ];
    if pane == DrawerPane::Settings {
        head_items.push(
            container(
                row![
                    button(text("基本設定").size(11.5).wrapping(Wrapping::None))
                        .padding([4, 9])
                        .style(theme::btn_tab(pal, !state.tuning_open))
                        .on_press(Message::SetTuningOpen(false)),
                    button(text("調整・計測").size(11.5).wrapping(Wrapping::None))
                        .padding([4, 9])
                        .style(theme::btn_tab(pal, state.tuning_open))
                        .on_press(Message::SetTuningOpen(true)),
                ]
                .spacing(2),
            )
            .padding(2)
            .style(theme::box_style(
                pal.bg_canvas,
                pal.border_subtle,
                9.0,
                1.0,
                None,
                Default::default(),
            ))
            .into(),
        );
    }
    head_items.push(icon_button(IconKind::Close, 30.0, 30.0, 13.5, pal, Message::CloseDrawer));

    let header = container(
        row(head_items)
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .center_y(54)
    .padding([0, 16])
    .clip(true)
    .style(theme::topbar_style(pal));

    let body_content: Element<'a, Message> = match pane {
        DrawerPane::Workers => render_workers_pane(state, pal),
        DrawerPane::Tools => render_tools_pane(state, pal),
        DrawerPane::Settings => {
            if state.tuning_open {
                render_tuning_pane(state, pal)
            } else {
                render_settings_pane(state, pal)
            }
        }
        DrawerPane::Files => render_files_pane(state, pal),
        DrawerPane::Sharing => render_sharing_pane(state, pal),
        DrawerPane::Logs => render_logs_pane(state, pal),
    };

    let body_scroll = scrollable(container(body_content).padding([14, 16]).width(Length::Fill))
        .direction(scrollable::Direction::Vertical(
            scrollable::Scrollbar::new()
                .width(6)
                .scroller_width(6)
                .spacing(6),
        ))
        .height(Length::Fill)
        .style(theme::scrollable_style(pal));

    let inner = container(column![header, rule::horizontal(1).style(theme::rule_subtle(pal)), body_scroll])
        .width(Length::Fixed(target_w))
        .height(Length::Fill)
        .style(theme::sidebar_style(pal));

    container(inner)
        .width(Length::Fixed(anim_w))
        .height(Length::Fill)
        .clip(true)
        .into()
}

fn section_card<'a>(title: &'static str, content: Element<'a, Message>, pal: UiPalette) -> Element<'a, Message> {
    container(
        column![
            text(title).size(12.5).font(UI_FONT_BOLD).color(pal.ink_secondary),
            content,
        ]
        .spacing(10),
    )
    .padding([12, 14])
    .width(Length::Fill)
    .style(theme::card_flat_style(pal))
    .into()
}

fn toggle_row<'a>(
    label: &'static str,
    checked: bool,
    on_toggle: Option<fn(bool) -> Message>,
    pal: UiPalette,
) -> Element<'a, Message> {
    row![
        text(label).size(13.0).color(pal.ink).width(Length::Fill),
        toggler(checked)
            .on_toggle_maybe(on_toggle)
            .size(18.0)
            .style(theme::toggler_style(pal)),
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

fn render_workers_pane<'a>(state: &'a UiState, pal: UiPalette) -> Element<'a, Message> {
    if state.workers_panels.is_empty() {
        return section_card(
            "8B 担当ワーカー",
            text("8B ワーカーなし")
                .size(12.5)
                .color(pal.ink_muted)
                .into(),
            pal,
        );
    }

    let cards = column(state.workers_panels.iter().enumerate().map(|(idx, p)| {
        let name = p
            .get("name")
            .or_else(|| p.get("title"))
            .or_else(|| p.get("role"))
            .and_then(|v| v.as_str())
            .unwrap_or("8B Worker")
            .to_string();
        let adapter = p
            .get("adapter")
            .or_else(|| p.get("model"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let status = p.get("status").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let tasks = p.get("tasks").and_then(|v| v.as_array()).map(|a| a.as_slice()).unwrap_or(&[]);

        let task_rows = column(tasks.iter().map(|t| {
            let title = t
                .get("title")
                .or_else(|| t.get("ownLabel"))
                .or_else(|| t.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("タスク")
                .to_string();
            let tst = t.get("status").and_then(|v| v.as_str()).unwrap_or("");
            let lbl = t.get("stateLabel").and_then(|v| v.as_str()).unwrap_or(tst).to_string();
            let step = t.get("stepLabel").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let st_c = match tst {
                "completed" => pal.green,
                "running" => pal.accent,
                "failed" => pal.red,
                _ => pal.ink_muted,
            };
            container(
                column![
                    row![
                        text(title).size(12.0).font(UI_FONT_BOLD).width(Length::Fill),
                        text(lbl).size(12.0).color(st_c),
                    ],
                    if step.is_empty() {
                        Element::from(space())
                    } else {
                        text(step).size(11.5).color(pal.ink_muted).into()
                    }
                ]
                .spacing(3),
            )
            .padding([6, 8])
            .width(Length::Fill)
            .style(theme::surface2_style(pal, 8.0))
            .into()
        }))
        .spacing(6);

        let _ = idx;
        container(
            column![
                row![
                    text(name).size(13.0).font(UI_FONT_BOLD),
                    text(adapter).size(11.0).color(pal.ink_muted),
                    space::horizontal(),
                    container(text(status).size(11.0).color(pal.ink_secondary))
                        .padding([2, 8])
                        .style(theme::pill_container_style(pal, theme::PillKind::Neutral)),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                task_rows,
            ]
            .spacing(8),
        )
        .padding([10, 12])
        .width(Length::Fill)
        .style(theme::card_flat_style(pal))
        .into()
    }))
    .spacing(10);

    cards.into()
}

fn render_tools_pane<'a>(state: &'a UiState, pal: UiPalette) -> Element<'a, Message> {
    let ws_on = state.gui.get("workspaceEnabled").and_then(|v| v.as_bool()) != Some(false);
    let cli_on = state.gui.get("localCliEnabled").and_then(|v| v.as_bool()) != Some(false);
    let run_on_pc = state.gui.get("runOnPc").and_then(|v| v.as_bool()).unwrap_or(false);
    let srv_win_avail = state
        .capabilities
        .get("serverWindows")
        .and_then(|s| s.get("available"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let pc_on = state.gui.get("pcEnabled").and_then(|v| v.as_bool()).unwrap_or(false);

    let wd_val = if !state.local_workdir.is_empty() {
        state.local_workdir.as_str()
    } else {
        state
            .gui
            .get("localWorkdir")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("(ジョブごとの自動ワークスペース)")
    };

    let mut ws_col = column![
        toggle_row("作業環境", ws_on, Some(Message::SetWorkspaceEnabled), pal),
        toggle_row("ローカルCLI", cli_on, Some(Message::SetLocalCli), pal),
    ]
    .spacing(8);
    if srv_win_avail {
        ws_col = ws_col.push(toggle_row(
            "ウィンドウ操作をこのPCで行う",
            run_on_pc,
            Some(Message::SetRunOnPc),
            pal,
        ));
    }
    ws_col = ws_col
        .push(text("作業フォルダー").size(12.0).color(pal.ink_muted))
        .push(
            container(text(wd_val.to_string()).size(12.5).color(pal.ink_secondary))
                .padding([7, 10])
                .width(Length::Fill)
                .style(theme::surface2_style(pal, 8.0)),
        )
        .push(
            row![
                button(text("フォルダー選択").size(12.0))
                    .padding([5, 10])
                    .style(theme::btn_secondary(pal))
                    .on_press(Message::PickWorkdir),
                button(text("エクスプローラー").size(12.0))
                    .padding([5, 10])
                    .style(theme::btn_secondary(pal))
                    .on_press(Message::OpenLocalFolder),
                button(text("標準に戻す").size(12.0))
                    .padding([5, 10])
                    .style(theme::btn_ghost(pal))
                    .on_press(Message::ResetWorkdir),
            ]
            .spacing(6),
        );

    let read_sec = {
        let mut rcol = column![
            row![
                text("読み取り専用 資料フォルダー").size(12.5).font(UI_FONT_BOLD).color(pal.ink_secondary),
                space::horizontal(),
                button(text("＋追加").size(11.5))
                    .padding([3, 8])
                    .style(theme::btn_secondary(pal))
                    .on_press(Message::AddReadFolder),
                button(text("全解除").size(11.5))
                    .padding([3, 8])
                    .style(theme::btn_ghost(pal))
                    .on_press(Message::ClearReadFolders),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
        ]
        .spacing(8);
        if state.read_roots.is_empty() {
            rcol = rcol.push(text("なし").size(12.0).color(pal.ink_muted));
        } else {
            for (idx, p) in state.read_roots.iter().enumerate() {
                rcol = rcol.push(
                    container(
                        row![
                            container(text(p.clone()).size(12.0).wrapping(Wrapping::None))
                                .width(Length::Fill)
                                .clip(true),
                            icon_button(IconKind::Close, 22.0, 22.0, 11.0, pal, Message::RemoveReadFolder(idx)),
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center),
                    )
                    .padding([5, 8])
                    .style(theme::surface2_style(pal, 7.0)),
                );
            }
        }
        container(rcol)
            .padding([12, 14])
            .width(Length::Fill)
            .style(theme::card_flat_style(pal))
    };

    let q = state.window_filter.to_ascii_lowercase();
    let wins: Vec<&Value> = state
        .pc_windows
        .iter()
        .filter(|w| {
            if q.is_empty() {
                return true;
            }
            let t = w.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let p = w.get("process_name").and_then(|v| v.as_str()).unwrap_or("");
            t.to_ascii_lowercase().contains(&q) || p.to_ascii_lowercase().contains(&q)
        })
        .collect();

    let win_list: Element<'a, Message> = if wins.is_empty() {
        text("なし").size(12.0).color(pal.ink_muted).into()
    } else {
        scrollable(
            column(wins.into_iter().map(|w| {
                let wid = w.get("id").and_then(|v| v.as_i64()).unwrap_or(0) as isize;
                let title = w.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let elev = w.get("elevated").and_then(|v| v.as_bool()).unwrap_or(false);
                let sel = wid == state.selected_window_id;
                button(
                    row![
                        text(title).size(12.5).width(Length::Fill),
                        if !state.elevated && elev {
                            Element::from(
                                container(text("管理者・観察可").size(10.5).color(pal.amber))
                                    .padding([2, 7])
                                    .style(theme::pill_container_style(pal, theme::PillKind::Warn)),
                            )
                        } else {
                            space().into()
                        }
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding([7, 10])
                .style(theme::btn_card(pal, sel))
                .on_press(Message::SelectPcWindow(wid))
                .into()
            }))
            .spacing(5),
        )
        .direction(scrollable::Direction::Vertical(
            scrollable::Scrollbar::new()
                .width(6)
                .scroller_width(6)
                .spacing(6),
        ))
        .height(Length::Fixed(210.0))
        .style(theme::scrollable_style(pal))
        .into()
    };

    let pc_sec = container(
        column![
            row![
                text("PC画面操作・連携待機").size(12.5).font(UI_FONT_BOLD).color(pal.ink_secondary),
                space::horizontal(),
                button(text("■ PC緊急停止").size(11.5))
                    .padding([4, 9])
                    .style(theme::btn_danger(pal))
                    .on_press(Message::StopPcAll),
            ]
            .align_y(Alignment::Center),
            toggle_row("PCウィンドウ操作を許可", pc_on, Some(Message::SetPcEnabled), pal),
            toggle_row(
                "スマホ・外部からのPC連携待機",
                state.standby,
                if state.connected { Some(Message::SetPcStandby) } else { None },
                pal
            ),
            row![
                text_input("ウィンドウを検索…", &state.window_filter)
                    .on_input(Message::WindowFilterChanged)
                    .size(12.5)
                    .padding([6, 10])
                    .style(theme::input_style(pal)),
                button(text("一覧更新").size(12.0))
                    .padding([6, 10])
                    .style(theme::btn_secondary(pal))
                    .on_press(Message::RefreshPcWindows),
            ]
            .spacing(6),
            win_list,
        ]
        .spacing(8),
    )
    .padding([12, 14])
    .width(Length::Fill)
    .style(theme::card_flat_style(pal));

    column![
        section_card("作業環境・ローカル操作", ws_col.into(), pal),
        read_sec,
        pc_sec,
    ]
    .spacing(12)
    .into()
}

fn render_settings_pane<'a>(state: &'a UiState, pal: UiPalette) -> Element<'a, Message> {
    let d = &state.settings_draft;
    let conn_card = section_card(
        "接続設定",
        column![
            text("HTTPS API URL").size(12.0).color(pal.ink_muted),
            text_input("https://localhost:8443", &d.api)
                .on_input(Message::DraftApi)
                .on_submit(Message::SaveAndConnect)
                .size(12.5)
                .padding([7, 10])
                .style(theme::input_style(pal)),
            text("初回接続時に設定 URL から https-server.json を自動生成します")
                .size(11.0)
                .color(pal.ink_muted),
            toggle_row("起動時に自動接続", d.auto_connect, Some(Message::DraftAutoConnect), pal),
            row![
                button(container(text("接続 / 更新").size(12.0).wrapping(Wrapping::None)).center_x(Length::Fill))
                    .width(Length::Fill)
                    .padding([6, 8])
                    .style(theme::btn_primary(pal))
                    .on_press(Message::SaveAndConnect),
                button(text("起動").size(12.0).wrapping(Wrapping::None))
                    .padding([6, 9])
                    .style(theme::btn_secondary(pal))
                    .on_press(Message::StartServer),
                button(text("再起動").size(12.0).wrapping(Wrapping::None))
                    .padding([6, 9])
                    .style(theme::btn_secondary(pal))
                    .on_press(Message::RestartServer),
                button(text("停止").size(12.0).wrapping(Wrapping::None))
                    .padding([6, 9])
                    .style(theme::btn_danger(pal))
                    .on_press(Message::StopServer),
            ]
            .spacing(6),
        ]
        .spacing(8)
        .into(),
        pal,
    );

    let inf_card = section_card(
        "推論・コンテキスト設定",
        column![
            row![
                column![
                    text("コンテキスト長").size(11.5).color(pal.ink_muted),
                    text_input("16384", &d.ctx)
                        .on_input(Message::DraftCtx)
                        .size(12.5)
                        .padding([6, 9])
                        .style(theme::input_style(pal)),
                ]
                .spacing(4)
                .width(Length::FillPortion(1)),
                column![
                    text("最大出力").size(11.5).color(pal.ink_muted),
                    text_input("1800", &d.tok)
                        .on_input(Message::DraftTok)
                        .size(12.5)
                        .padding([6, 9])
                        .style(theme::input_style(pal)),
                ]
                .spacing(4)
                .width(Length::FillPortion(1)),
            ]
            .spacing(8),
            row![
                column![
                    text("推論強度").size(11.5).color(pal.ink_muted),
                    pick_list(EffortOpt::ALL, Some(d.effort), Message::DraftEffort)
                        .width(Length::Fill)
                        .text_size(12.5)
                        .padding([6, 9])
                        .style(theme::pick_list_style(pal))
                        .menu_style(theme::pick_menu_style(pal)),
                ]
                .spacing(4)
                .width(Length::FillPortion(1)),
                column![
                    text("推論予算").size(11.5).color(pal.ink_muted),
                    text_input("-1", &d.budget)
                        .on_input(Message::DraftBudget)
                        .size(12.5)
                        .padding([6, 9])
                        .style(theme::input_style(pal)),
                ]
                .spacing(4)
                .width(Length::FillPortion(1)),
            ]
            .spacing(8),
            button(container(text("適用・保存").size(12.5)).center_x(Length::Fill))
                .width(Length::Fill)
                .padding([7, 12])
                .style(theme::btn_primary(pal))
                .on_press(Message::ApplyServerSettings),
        ]
        .spacing(8)
        .into(),
        pal,
    );

    column![conn_card, inf_card].spacing(12).into()
}

fn slider_row<'a>(
    label: &'static str,
    val: f64,
    range: std::ops::RangeInclusive<f64>,
    step: f64,
    decimals: usize,
    on_change: fn(f64) -> Message,
    pal: UiPalette,
) -> Element<'a, Message> {
    let val_txt = format!("{:.prec$}", val, prec = decimals);
    column![
        row![
            text(label).size(12.0).color(pal.ink),
            space::horizontal(),
            text(val_txt).size(12.0).font(MONO_FONT).color(pal.accent),
        ],
        slider(range, val, on_change)
            .step(step)
            .style(theme::slider_style(pal)),
    ]
    .spacing(3)
    .into()
}

fn render_tuning_pane<'a>(state: &'a UiState, pal: UiPalette) -> Element<'a, Message> {
    let d = &state.settings_draft;
    let p_name = d.sampling_preset.as_str();

    let presets = column![
        row![
            preset_btn("コード・厳密 (T=0.05)", "code", p_name == "code", pal),
            preset_btn("標準バランス (T=0.20)", "balanced", p_name == "balanced", pal),
        ]
        .spacing(6),
        row![
            preset_btn("創造・多様 (T=0.70)", "creative", p_name == "creative", pal),
            preset_btn("カスタム", "custom", p_name == "custom", pal),
        ]
        .spacing(6),
    ]
    .spacing(6);

    let sampling_card = container(
        column![
            row![
                text("サンプリングプリセット").size(12.5).font(UI_FONT_BOLD).color(pal.ink_secondary),
                space::horizontal(),
                button(text("既定に戻す").size(11.5))
                    .padding([3, 8])
                    .style(theme::btn_ghost(pal))
                    .on_press(Message::ResetSampling),
            ]
            .align_y(Alignment::Center),
            presets,
            slider_row("Temperature", d.temperature, 0.0..=2.0, 0.05, 2, Message::SliderTemp, pal),
            slider_row("Top P", d.top_p, 0.0..=1.0, 0.01, 2, Message::SliderTopP, pal),
            column![
                row![
                    text("Top K").size(12.0).color(pal.ink),
                    space::horizontal(),
                    text(d.top_k.to_string()).size(12.0).font(MONO_FONT).color(pal.accent),
                ],
                slider(1..=100, d.top_k, Message::SliderTopK)
                    .step(1)
                    .style(theme::slider_style(pal)),
            ]
            .spacing(3),
            slider_row("Min P", d.min_p, 0.0..=1.0, 0.01, 2, Message::SliderMinP, pal),
            slider_row("Repeat Penalty", d.repeat_penalty, 1.0..=2.0, 0.01, 2, Message::SliderRepeat, pal),
            slider_row("Presence Penalty", d.presence_penalty, -2.0..=2.0, 0.05, 2, Message::SliderPresence, pal),
            slider_row("Frequency Penalty", d.frequency_penalty, -2.0..=2.0, 0.05, 2, Message::SliderFrequency, pal),
            button(container(text("サンプリングを適用").size(12.5)).center_x(Length::Fill))
                .width(Length::Fill)
                .padding([7, 12])
                .style(theme::btn_primary(pal))
                .on_press(Message::ApplySampling),
        ]
        .spacing(8),
    )
    .padding([12, 14])
    .width(Length::Fill)
    .style(theme::card_flat_style(pal));

    let vram_used = state
        .perf
        .get("vramUsedMb")
        .or_else(|| state.perf.get("vramUsedMB"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let vram_tot = state
        .perf
        .get("vramTotalMb")
        .or_else(|| state.perf.get("vramTotalMB"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let vram_ratio = if vram_tot > 0.0 {
        (vram_used / vram_tot) as f32
    } else {
        0.0
    };
    let tps = state
        .perf
        .get("tokensPerSec")
        .or_else(|| state.perf.get("lastTokensPerSec"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let at_run = state.autotune.get("running").and_then(|v| v.as_bool()).unwrap_or(false);
    let at_err = state.autotune.get("error").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let at_stage = state.autotune.get("stage").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let at_pct = state.autotune.get("progressPercent").and_then(|v| v.as_i64()).unwrap_or(0);

    let mut perf_col = column![
        row![
            text("推論パフォーマンス計測").size(12.5).font(UI_FONT_BOLD).color(pal.ink_secondary),
            space::horizontal(),
            button(text("再計測").size(11.5))
                .padding([3, 8])
                .style(theme::btn_secondary(pal))
                .on_press(Message::RefreshPerf),
        ]
        .align_y(Alignment::Center),
        text(format!(
            "速度: {}{}",
            if tps > 0.0 { format!("{:.1} tok/s", tps) } else { "未計測".into() },
            if vram_tot > 0.0 {
                format!(" · VRAM: {:.0} / {:.0} MB ({:.0}%)", vram_used, vram_tot, vram_ratio * 100.0)
            } else {
                String::new()
            }
        ))
        .size(12.0),
    ]
    .spacing(8);

    if vram_tot > 0.0 {
        perf_col = perf_col.push(meter_bar(vram_ratio, pal));
    }
    perf_col = perf_col
        .push(text("自動最適化 (AutoTune)").size(12.0).color(pal.ink_muted))
        .push(
            row![
                pick_list(TuneModeOpt::ALL, Some(d.tune_mode), Message::SelectTuneMode)
                    .width(Length::Fill)
                    .text_size(12.5)
                    .padding([6, 9])
                    .style(theme::pick_list_style(pal))
                    .menu_style(theme::pick_menu_style(pal)),
                button(text(if at_run { "計測中…" } else { "最適化を実行" }).size(12.0))
                    .padding([6, 12])
                    .style(theme::btn_secondary(pal))
                    .on_press_maybe((!at_run).then_some(Message::StartAutoTune)),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        );
    if !at_err.is_empty() {
        perf_col = perf_col.push(text(at_err).size(11.5).color(pal.red));
    } else if !at_stage.is_empty() {
        perf_col = perf_col.push(
            text(format!("{} ({}%)", at_stage, at_pct))
                .size(11.5)
                .color(pal.ink_secondary),
        );
    }

    column![
        sampling_card,
        container(perf_col)
            .padding([12, 14])
            .width(Length::Fill)
            .style(theme::card_flat_style(pal)),
    ]
    .spacing(12)
    .into()
}

fn preset_btn<'a>(label: &'static str, name: &'static str, active: bool, pal: UiPalette) -> Element<'a, Message> {
    button(text(label).size(12.0))
        .width(Length::FillPortion(1))
        .padding([7, 9])
        .style(theme::btn_card(pal, active))
        .on_press_maybe((name != "custom").then(|| Message::SelectPreset(name.to_string())))
        .into()
}

fn render_files_pane<'a>(state: &'a UiState, pal: UiPalette) -> Element<'a, Message> {
    let has_conv = state.conversation.as_ref().and_then(|c| c.get("id")).is_some();
    let head = row![
        text("ワークスペース・成果物")
            .size(12.5)
            .font(UI_FONT_BOLD)
            .color(pal.ink_secondary)
            .wrapping(Wrapping::None),
        space::horizontal(),
        button(
            row![
                icon(IconKind::Refresh, 12.0, pal.ink_secondary),
                text("更新").size(11.5).wrapping(Wrapping::None),
            ]
            .spacing(5)
            .align_y(Alignment::Center),
        )
        .padding([4, 8])
        .style(theme::btn_secondary(pal))
        .on_press_maybe(has_conv.then_some(Message::FilesRefresh)),
        button(
            row![
                icon(IconKind::Download, 12.0, iced::Color::WHITE),
                text("一括保存").size(11.5).wrapping(Wrapping::None),
            ]
            .spacing(5)
            .align_y(Alignment::Center),
        )
        .padding([4, 8])
        .style(theme::btn_primary(pal))
        .on_press_maybe(has_conv.then_some(Message::SaveDownloads(None))),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let body: Element<'a, Message> = if !has_conv {
        text("ファイルなし").size(12.0).color(pal.ink_muted).into()
    } else if state.files_loading {
        shimmer_skeleton(
            state.anim.shimmer_phase,
            vec![(0.95, 34.0), (0.88, 34.0), (0.92, 34.0), (0.76, 34.0)],
            8.0,
            10.0,
            pal,
        )
    } else if state.files_items.is_empty() {
        text("ファイルなし").size(12.0).color(pal.ink_muted).into()
    } else {
        column(state.files_items.iter().map(|item| {
            let path = item.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let bytes = item.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0);
            button(
                row![
                    text(format!("📄 {}", path)).size(12.5).width(Length::Fill),
                    text(format!("{} B", bytes)).size(11.0).color(pal.ink_muted),
                ]
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([8, 10])
            .style(theme::btn_card(pal, false))
            .on_press(Message::FilesPreview(path))
            .into()
        }))
        .spacing(6)
        .into()
    };

    container(column![head, body].spacing(10))
        .padding([12, 14])
        .width(Length::Fill)
        .style(theme::card_flat_style(pal))
        .into()
}

fn render_sharing_pane<'a>(state: &'a UiState, pal: UiPalette) -> Element<'a, Message> {
    let has_conv = state.conversation.as_ref().and_then(|c| c.get("id")).is_some();
    let d = &state.settings_draft;
    let form = row![
        text_input("192.0.2.50", &d.share_ip)
            .on_input(Message::DraftShareIp)
            .size(12.5)
            .padding([6, 9])
            .style(theme::input_style(pal)),
        pick_list(ShareModeOpt::ALL, Some(d.share_mode), Message::DraftShareMode)
            .text_size(12.0)
            .padding([6, 9])
            .style(theme::pick_list_style(pal))
            .menu_style(theme::pick_menu_style(pal)),
        button(text("共有").size(12.0))
            .padding([6, 11])
            .style(theme::btn_primary(pal))
            .on_press_maybe(has_conv.then_some(Message::ShareAdd)),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let list: Element<'a, Message> = if state.sharing_entries.is_empty() {
        text("共有設定なし")
            .size(12.0)
            .color(pal.ink_muted)
            .into()
    } else {
        column(state.sharing_entries.iter().enumerate().map(|(idx, e)| {
            let ip = e.get("ip").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let mode = e.get("mode").and_then(|v| v.as_str()).unwrap_or("content");
            container(
                row![
                    text(format!("{} ({})", ip, mode)).size(12.5).width(Length::Fill),
                    button(text("解除").size(11.5))
                        .padding([3, 8])
                        .style(theme::btn_danger(pal))
                        .on_press(Message::ShareRemove(idx)),
                ]
                .align_y(Alignment::Center),
            )
            .padding([6, 8])
            .style(theme::surface2_style(pal, 7.0))
            .into()
        }))
        .spacing(6)
        .into()
    };

    section_card("IP別アクセス共有", column![form, list].spacing(10).into(), pal)
}

fn render_logs_pane<'a>(state: &'a UiState, pal: UiPalette) -> Element<'a, Message> {
    let f = state.log_filter.to_ascii_lowercase();
    let visible: Vec<&(String, String)> = state
        .logs
        .iter()
        .filter(|(line, _)| f.is_empty() || line.to_ascii_lowercase().contains(&f))
        .collect();
    let start = visible.len().saturating_sub(250);

    let bar = row![
        text_input("ログを絞り込み…", &state.log_filter)
            .on_input(Message::LogFilterChanged)
            .size(12.5)
            .padding([6, 9])
            .style(theme::input_style(pal)),
        button(text("コピー").size(12.0))
            .padding([6, 10])
            .style(theme::btn_secondary(pal))
            .on_press(Message::CopyVisibleLogs),
    ]
    .spacing(6);

    let rows: Element<'a, Message> = if visible.is_empty() {
        text("ログなし").size(12.0).color(pal.ink_muted).into()
    } else {
        column(visible[start..].iter().map(|(line, level)| {
            let (bg, fg) = match level.as_str() {
                "warn" => (pal.amber_soft, pal.amber),
                "error" => (pal.red_soft, pal.red),
                _ => (iced::Color::TRANSPARENT, pal.ink_secondary),
            };
            container(text(line.trim_end().to_string()).size(11.5).font(MONO_FONT).color(fg))
                .padding([3, 6])
                .width(Length::Fill)
                .style(theme::box_style(bg, iced::Color::TRANSPARENT, 6.0, 0.0, Some(fg), Default::default()))
                .into()
        }))
        .spacing(3)
        .into()
    };

    column![bar, rows].spacing(10).into()
}
