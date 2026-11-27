use crate::live_status::format_turn_metrics;
use crate::ui::anim::{
    chevron_icon, pulse_dot, shimmer_skeleton, thinking_dots, top_progress_bar,
};
use crate::ui::app::{DrawerPane, UiState, WorkModeOpt};
use crate::ui::icons::{icon, icon_button, IconKind};
use crate::ui::markdown_view;
use crate::ui::selectable::SelectableText;
use crate::ui::theme::{self, MONO_FONT, PillKind, UI_FONT, UI_FONT_BOLD, UiPalette};
use crate::ui::Message;
use iced::keyboard;
use iced::widget::text::Wrapping;
use iced::widget::{
    button, column, container, image as img_w, pick_list, row, rule, scrollable, space, text,
    text_editor, text_input,
};
use iced::{Alignment, ContentFit, Element, Length, Padding};
use serde_json::Value;

pub struct SlashOption {
    pub cmd: &'static str,
    pub label: &'static str,
    pub desc: &'static str,
    pub disabled: bool,
}

pub const SLASH_ITEMS: &[SlashOption] = &[
    SlashOption { cmd: "/plan ", label: "/plan <task>", desc: "事前調査と実行計画 (Direct 27B)", disabled: false },
    SlashOption { cmd: "/goal ", label: "/goal <task>", desc: "検証完了まで自律ループで徹底遂行", disabled: false },
    SlashOption { cmd: "!compact", label: "/compact", desc: "会話履歴を要約・圧縮", disabled: false },
    SlashOption { cmd: "/grill-me ", label: "/grill-me <topic>", desc: "実装前の仕様曖昧さを質問で明確化", disabled: false },
    SlashOption { cmd: "/boost ", label: "/boost <task>", desc: "推論強化・多角検証で解決", disabled: false },
    SlashOption { cmd: "/teamwork ", label: "/teamwork <task>", desc: "設計→実装→検証の工程分離実行 (Direct 27B)", disabled: false },
    SlashOption { cmd: "/learn ", label: "/learn <note|list>", desc: "知見を学習メモリへ保存・一覧表示", disabled: false },
    SlashOption { cmd: "/browser ", label: "/browser <task>", desc: "Web検索・画面確認を優先実行", disabled: false },
    SlashOption { cmd: "/schedule ", label: "/schedule <in 10m|every 1h|list>", desc: "遅延・定期ジョブ登録・管理", disabled: false },
    SlashOption { cmd: "/mode ", label: "/mode <standard|chat|coding|pc_control>", desc: "動作モードを切り替え", disabled: false },
    SlashOption { cmd: "/effort ", label: "/effort <none|low|medium|xhigh>", desc: "推論負荷を設定", disabled: false },
    SlashOption { cmd: "!status", label: "/status", desc: "サーバー・モデル・予定の現在状態", disabled: false },
    SlashOption { cmd: "!new", label: "/clear", desc: "会話をクリアして新規チャット", disabled: false },
    SlashOption { cmd: "!help", label: "/help", desc: "agy / 作業環境コマンド一覧", disabled: false },
    SlashOption { cmd: "/workdir ", label: "/workdir <フォルダー>", desc: "このチャットの作業フォルダーを変更", disabled: false },
    SlashOption { cmd: "/reset", label: "/reset", desc: "作業フォルダー設定を標準へ戻す", disabled: false },
    SlashOption { cmd: "!new", label: "/new", desc: "新しいチャットを作成", disabled: false },
    SlashOption { cmd: "!connect", label: "/connect", desc: "サーバーへ接続・再読み込み", disabled: false },
    SlashOption { cmd: "!pane:workers", label: "/workers", desc: "8B担当パネル（利用不可）", disabled: true },
    SlashOption { cmd: "!pane:tools", label: "/tools", desc: "作業環境・PC操作を開く", disabled: false },
    SlashOption { cmd: "!pane:settings", label: "/settings", desc: "設定パネルを開く", disabled: false },
];

pub fn filtered_slash_items(prompt: &str) -> Vec<&'static SlashOption> {
    if !prompt.starts_with('/') || prompt.contains('\n') || prompt.contains(' ') || prompt.contains('\t') {
        return Vec::new();
    }
    let q = prompt.to_ascii_lowercase();
    SLASH_ITEMS
        .iter()
        .filter(|it| {
            if q == "/" {
                return true;
            }
            let name = it.label.split_whitespace().next().unwrap_or(it.label).to_ascii_lowercase();
            name.starts_with(&q)
        })
        .collect()
}

fn status_jp(s: &str) -> &str {
    match s {
        "running" => "実行中",
        "queued" => "待機中",
        "waiting_user" => "質問への回答待ち",
        "completed" => "完了",
        "cancelled" => "中断",
        "interrupted" => "停止",
        "failed" => "失敗",
        _ => s,
    }
}

fn model_badge(job: &Value) -> &'static str {
    let opts = job.get("options");
    let role = opts.and_then(|o| o.get("modelRole")).and_then(|v| v.as_str()).unwrap_or("");
    let wm = opts.and_then(|o| o.get("workMode")).and_then(|v| v.as_str()).unwrap_or("");
    let pm = opts.and_then(|o| o.get("planMode")).and_then(|v| v.as_bool()).unwrap_or(false);
    if role == "worker" {
        "8B + LoRA"
    } else if wm == "pc_control" {
        "Qwen27 · PC操作"
    } else if wm == "chat" {
        "Qwen27 · チャット"
    } else if pm {
        "Qwen27 · Plan委譲"
    } else {
        "Qwen27 · Direct"
    }
}

pub fn render_main_workspace<'a>(state: &'a UiState, pal: UiPalette) -> Element<'a, Message> {
    let conv_title = state
        .conversation
        .as_ref()
        .and_then(|c| c.get("title"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("新しいチャット")
        .to_string();

    let tabs = [
        (DrawerPane::Sharing, "共有"),
        (DrawerPane::Tools, "作業環境"),
        (DrawerPane::Files, "ファイル"),
        (DrawerPane::Logs, "ログ"),
        (DrawerPane::Workers, "8B担当"),
        (DrawerPane::Settings, "設定"),
    ];
    let segmented = container(
        row(tabs.into_iter().map(|(pane, label)| {
            let disabled = pane == DrawerPane::Workers;
            let active = !disabled && state.active_pane == Some(pane);
            button(
                text(label)
                    .size(12.0)
                    .font(UI_FONT_BOLD)
                    .wrapping(Wrapping::None),
            )
            .padding([5, 9])
            .style(theme::btn_tab(pal, active))
            .on_press_maybe((!disabled).then_some(Message::TogglePane(pane)))
            .into()
        }))
        .spacing(2)
        .align_y(Alignment::Center),
    )
    .padding(3)
    .style(theme::box_style(pal.bg_surface, pal.border_subtle, 11.0, 1.0, None, Default::default()));

    let cmd_btn = {
        let mut cmd_row: Vec<Element<'a, Message>> = vec![
            icon(IconKind::Command, 13.0, pal.ink_secondary),
        ];
        if state.active_pane.is_none() {
            cmd_row.push(text("コマンド").size(12.0).wrapping(Wrapping::None).into());
        }
        cmd_row.push(
            container(
                text("Ctrl+K")
                    .size(10.0)
                    .font(MONO_FONT)
                    .color(pal.ink_muted)
                    .wrapping(Wrapping::None),
            )
            .padding([1, 5])
            .style(theme::surface2_style(pal, 5.0))
            .into(),
        );
        button(row(cmd_row).spacing(5).align_y(Alignment::Center))
            .padding([5, 8])
            .style(theme::btn_ghost(pal))
            .on_press(Message::OpenModal(crate::ui::app::ModalKind::Palette))
    };

    let mut topbar_items: Vec<Element<'a, Message>> = vec![
        icon_button(IconKind::Menu, 32.0, 32.0, 15.0, pal, Message::ToggleSidebar),
        container(
            text(conv_title)
                .size(14.5)
                .font(UI_FONT_BOLD)
                .wrapping(Wrapping::None),
        )
        .width(Length::Fill)
        .clip(true)
        .into(),
    ];
    if state.read_only {
        topbar_items.push(
            container(
                text("閲覧専用")
                    .size(11.0)
                    .font(UI_FONT_BOLD)
                    .color(pal.amber)
                    .wrapping(Wrapping::None),
            )
            .padding([2, 8])
            .style(theme::pill_container_style(pal, PillKind::Warn))
            .into(),
        );
    }
    topbar_items.push(cmd_btn.into());
    topbar_items.push(segmented.into());

    let topbar = container(
        row(topbar_items)
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .center_y(54)
    .padding([0, 14])
    .clip(true)
    .style(theme::topbar_style(pal));

    let progress = top_progress_bar(state.anim.shimmer_phase, state.anim.loading_bar.value(), pal);

    let (dot_c, dot_anim) = if state.busy || state.connecting {
        (pal.accent, true)
    } else if state.standby {
        (pal.amber, true)
    } else if state.connected {
        (pal.green, false)
    } else {
        (pal.ink_muted, false)
    };
    let mut status_items: Vec<Element<'a, Message>> = vec![
        pulse_dot(state.anim.pulse_phase, dot_c, dot_anim),
        container(
            text(state.status_main.clone())
                .size(12.0)
                .font(UI_FONT_BOLD)
                .color(pal.ink_secondary)
                .wrapping(Wrapping::None),
        )
        .width(Length::Fill)
        .clip(true)
        .into(),
    ];
    if !state.status_tail.is_empty() {
        status_items.push(
            text(state.status_tail.clone())
                .size(11.5)
                .color(if state.status_stale { pal.amber } else { pal.ink_muted })
                .wrapping(Wrapping::None)
                .into(),
        );
    }
    status_items.push(
        button(
            row![
                icon(IconKind::Search, 12.0, pal.ink_secondary),
                text("検索 Ctrl+F").size(11.5).wrapping(Wrapping::None),
            ]
            .spacing(5)
            .align_y(Alignment::Center),
        )
        .padding([4, 8])
        .style(theme::btn_ghost(pal))
        .on_press(Message::ToggleFind)
        .into(),
    );

    let status_strip = container(
        row(status_items)
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .center_y(34)
    .padding([0, 16])
    .clip(true)
    .style(theme::status_strip_style(pal));

    let find_bar: Element<'a, Message> = if state.anim.find_bar.is_visible() {
        let fh = 44.0 * state.anim.find_bar.value();
        let hits = state.find_match_jobs();
        let count_txt = if state.find_query.trim().is_empty() {
            String::new()
        } else if hits.is_empty() {
            "0 件".to_string()
        } else {
            format!("{} / {} 件", state.find_match_idx + 1, hits.len())
        };
        let bar_inner = container(
            row![
                text_input("会話内を検索…", &state.find_query)
                    .id("find-input")
                    .on_input(Message::FindQueryChanged)
                    .on_submit(Message::StepFind(1))
                    .size(13.0)
                    .padding([5, 10])
                    .style(theme::input_style(pal)),
                text(count_txt).size(12.0).color(pal.ink_muted).wrapping(Wrapping::None),
                button(text("前へ").size(12.0).wrapping(Wrapping::None))
                    .padding([4, 9])
                    .style(theme::btn_ghost(pal))
                    .on_press(Message::StepFind(-1)),
                button(text("次へ").size(12.0).wrapping(Wrapping::None))
                    .padding([4, 9])
                    .style(theme::btn_ghost(pal))
                    .on_press(Message::StepFind(1)),
                icon_button(IconKind::Close, 26.0, 26.0, 12.0, pal, Message::ToggleFind),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .center_y(44)
        .padding([0, 16])
        .clip(true)
        .style(theme::card_flat_style(pal));
        container(bar_inner)
            .height(Length::Fixed(fh))
            .width(Length::Fill)
            .clip(true)
            .into()
    } else {
        space().into()
    };

    let body = render_viewport(state, pal);
    let task_strip = render_task_strip(state, pal);
    let question_panel = render_question_panel(state, pal);
    let notice_banner = render_notice_banner(state, pal);
    let attach_tray = render_attach_tray(state, pal);
    let composer = render_composer(state, pal);

    container(column![
        topbar,
        container(progress).width(Length::Fill),
        rule::horizontal(1).style(theme::rule_subtle(pal)),
        status_strip,
        rule::horizontal(1).style(theme::rule_subtle(pal)),
        container(find_bar).width(Length::Fill),
        body,
        task_strip,
        question_panel,
        notice_banner,
        attach_tray,
        composer,
    ])
    .width(Length::Fill)
    .height(Length::Fill)
    .style(theme::canvas_style(pal))
    .into()
}

fn render_viewport<'a>(state: &'a UiState, pal: UiPalette) -> Element<'a, Message> {
    let jobs = state
        .conversation
        .as_ref()
        .and_then(|c| c.get("jobs"))
        .and_then(|v| v.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[]);

    if state.loading_conversation {
        let skel = container(
            column![
                row![space::horizontal(), container(shimmer_skeleton(state.anim.shimmer_phase, vec![(1.0, 44.0)], 6.0, 14.0, pal)).width(Length::Fixed(280.0))],
                container(
                    shimmer_skeleton(
                        state.anim.shimmer_phase,
                        vec![(0.35, 18.0), (0.95, 16.0), (0.88, 16.0), (0.62, 16.0), (0.90, 68.0)],
                        10.0,
                        10.0,
                        pal,
                    )
                )
                .padding(20)
                .style(theme::card_style(pal)),
            ]
            .spacing(18)
            .max_width(840),
        )
        .center_x(Length::Fill)
        .padding([24, 24]);
        return scrollable(skel)
            .height(Length::Fill)
            .style(theme::scrollable_style(pal))
            .into();
    }

    if jobs.is_empty() {
        return scrollable(
            container(render_welcome(state, pal))
                .center_x(Length::Fill)
                .padding([28, 24]),
        )
        .height(Length::Fill)
        .style(theme::scrollable_style(pal))
        .into();
    }

    let find_hits = state.find_match_jobs();
    let highlighted_job_id = find_hits.get(state.find_match_idx).map(|s| s.as_str());

    let turns = column(jobs.iter().map(|job| render_job_turn(state, job, highlighted_job_id, pal)))
        .spacing(18)
        .max_width(840);

    let sc = scrollable(
        container(turns)
            .center_x(Length::Fill)
            .padding([20, 24]),
    )
    .id(state.scroll_id.clone())
    .direction(scrollable::Direction::Vertical(
        scrollable::Scrollbar::new()
            .width(6)
            .scroller_width(6)
            .spacing(6),
    ))
    .height(Length::Fill)
    .on_scroll(|vp| Message::ChatScrolled {
        abs_y: vp.absolute_offset().y,
        dist_from_bottom: vp.absolute_offset_reversed().y,
        content_h: vp.content_bounds().height,
        viewport_h: vp.bounds().height,
    })
    .style(theme::scrollable_style(pal));

    let jump_layer: Element<'a, Message> = if !state.at_bottom {
        container(
            button(
                row![
                    icon(IconKind::ArrowDown, 12.0, pal.ink),
                    text("最新へ")
                        .size(12.0)
                        .font(UI_FONT_BOLD)
                        .wrapping(Wrapping::None),
                ]
                .spacing(5)
                .align_y(Alignment::Center),
            )
            .padding([6, 14])
            .style(theme::btn_secondary(pal))
            .on_press(Message::ScrollToBottom),
        )
        .align_bottom(Length::Fill)
        .center_x(Length::Fill)
        .padding(Padding { top: 0.0, right: 0.0, bottom: 10.0, left: 0.0 })
        .into()
    } else {
        space().into()
    };

    iced::widget::stack![sc, jump_layer]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn render_welcome<'a>(state: &'a UiState, pal: UiPalette) -> Element<'a, Message> {
    let wm = state.gui.get("workMode").and_then(|v| v.as_str()).unwrap_or("standard");
    let ws_on = state.gui.get("workspaceEnabled").and_then(|v| v.as_bool()).unwrap_or(true);
    let pc_on = state.gui.get("pcEnabled").and_then(|v| v.as_bool()).unwrap_or(false);

    let starters: Vec<(&str, &str)> = match wm {
        "chat" => vec![
            ("設計レビュー・比較相談", "次の案について技術的なメリット・デメリットと推奨案を比較してください："),
            ("文章校正・トーン調整", "次の文章をより簡潔で誤解のない表現に直してください："),
        ],
        "coding" => vec![
            ("バグ調査と修正 + テスト", "不具合の原因を調査し、最小の差分で修正した上でテストを実行して確認してください："),
            ("リファクタリング", "対象コードの可読性と保守性を改善し、ビルド・テストが通ることを確認してください。"),
        ],
        "pc_control" => vec![
            ("現在の画面状態を観察", "選択中のウィンドウを撮影・観察し、現在の画面状態と次に取れる操作を教えてください。"),
            ("画面を見ながら手順実行", "選択中のウィンドウを操作して次の目標を達成してください："),
        ],
        _ => vec![
            ("コード・資料の調査と要約", "選択した作業フォルダーの構成を確認し、重要なポイントと改善案をまとめてください。"),
            ("スクリプト・ツールの作成", "目的のツールを新規作成し、テストを実行して結果まで確認してください。"),
            ("長文ドキュメント整理", "以下の要件をもとに、見出し・表・手順を含むわかりやすい設計メモを作成してください："),
        ],
    };

    let mut pills = row![
        container(
            text(if state.connected { "✓ サーバー接続中" } else { "○ 未接続" })
                .size(12.0)
                .font(UI_FONT_BOLD)
        )
        .padding([5, 11])
        .style(theme::pill_container_style(
            pal,
            if state.connected { PillKind::Ok } else { PillKind::Neutral },
        )),
        container(
            text("27B Direct")
                .size(12.0)
                .font(UI_FONT_BOLD)
        )
        .padding([5, 11])
        .style(theme::pill_container_style(pal, PillKind::Accent)),
    ]
    .spacing(8);

    if ws_on {
        let wd = state
            .gui
            .get("localWorkdir")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("自動ワークスペース");
        pills = pills.push(
            container(text(format!("作業環境: {}", wd)).size(12.0))
                .padding([5, 11])
                .style(theme::pill_container_style(pal, PillKind::Neutral)),
        );
    }
    if pc_on && !state.selected_window_title.is_empty() {
        pills = pills.push(
            container(text(format!("PC対象: {}", state.selected_window_title)).size(12.0))
                .padding([5, 11])
                .style(theme::pill_container_style(pal, PillKind::Accent)),
        );
    }
    if state.standby {
        pills = pills.push(
            container(text("PC連携待機中").size(12.0))
                .padding([5, 11])
                .style(theme::pill_container_style(pal, PillKind::Ok)),
        );
    }

    let starter_cards = row(starters.into_iter().map(|(t, prompt)| {
        button(
            text(t).size(13.0).font(UI_FONT_BOLD).color(pal.ink),
        )
        .width(Length::FillPortion(1))
        .padding([13, 14])
        .style(theme::btn_card(pal, false))
        .on_press(Message::InsertStarter(prompt.to_string()))
        .into()
    }))
    .spacing(10);

    container(
        column![
            text("Qwen27").size(22.0).font(UI_FONT_BOLD),
            pills.wrap(),
            starter_cards,
        ]
        .spacing(12),
    )
    .max_width(780)
    .width(Length::Fill)
    .padding(30)
    .style(theme::card_style(pal))
    .into()
}

fn render_job_turn<'a>(
    state: &'a UiState,
    job: &'a Value,
    highlighted_job_id: Option<&str>,
    pal: UiPalette,
) -> Element<'a, Message> {
    let jid = job.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let status = job.get("status").and_then(|v| v.as_str()).unwrap_or("");
    let is_pending = matches!(status, "running" | "queued");
    let goal = job.get("goal").and_then(|v| v.as_str()).unwrap_or("");
    let is_find_hi = highlighted_job_id == Some(jid);

    let conv_id = job
        .get("conversationId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            state
                .conversation
                .as_ref()
                .and_then(|c| c.get("id"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("")
        .to_string();

    let files = job
        .get("attachments")
        .or_else(|| job.get("files"))
        .and_then(|v| v.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[]);
    let mut user_col = column![
        SelectableText::new(goal.to_string())
            .font(UI_FONT)
            .size(14.0)
            .color(pal.ink)
            .selection_color(pal.accent.scale_alpha(0.32))
            .width(Length::Shrink)
            .wrapping(Wrapping::WordOrGlyph)
            .scope(state.selection_scope.clone())
            .on_copy(|c| Message::SelectionCopied(c)),
    ]
    .spacing(8);
    if !files.is_empty() {
        let chips = row(files.iter().map(|f| {
            let raw_path = f
                .get("path")
                .or_else(|| f.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let nm = f
                .get("displayName")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    raw_path
                        .strip_prefix("uploads/")
                        .unwrap_or(if raw_path.is_empty() { "file" } else { raw_path.as_str() })
                        .to_string()
                });
            let thumb = (!raw_path.is_empty())
                .then(|| {
                    (!conv_id.is_empty())
                        .then(|| state.sent_attach_thumbs.get(&format!("{}:{}", conv_id, raw_path)))
                        .flatten()
                        .or_else(|| state.sent_attach_thumbs.get(&raw_path))
                })
                .flatten();
            let card_content: Element<'a, Message> = if let Some(handle) = thumb {
                column![
                    img_w(handle.clone())
                        .width(148)
                        .height(96)
                        .content_fit(ContentFit::Cover),
                    container(
                        text(nm.clone())
                            .size(11.0)
                            .color(pal.ink_secondary)
                            .wrapping(Wrapping::None),
                    )
                    .width(Length::Fixed(148.0))
                    .clip(true),
                ]
                .spacing(4)
                .into()
            } else {
                row![
                    icon(IconKind::Paperclip, 12.0, pal.ink_secondary),
                    text(nm.clone()).size(11.5).color(pal.ink),
                ]
                .spacing(5)
                .align_y(Alignment::Center)
                .into()
            };
            button(card_content)
                .padding(if thumb.is_some() { [6, 6] } else { [4, 10] })
                .style(theme::btn_card(pal, false))
                .on_press_maybe((!raw_path.is_empty()).then(|| Message::PreviewSentAttachment {
                    conv_id: conv_id.clone(),
                    path: raw_path,
                    display_name: nm,
                }))
                .into()
        }))
        .spacing(6)
        .wrap();
        user_col = user_col.push(chips);
    }
    let user_row = row![
        space::horizontal(),
        container(user_col)
            .max_width(640)
            .padding([12, 16])
            .style(theme::user_bubble_style(pal)),
    ];

    let act_rows = job.get("activity").and_then(|v| v.as_array()).map(|a| a.as_slice()).unwrap_or(&[]);
    let act_group: Element<'a, Message> = if !act_rows.is_empty() {
        let key = format!("act:{}", jid);
        let prog = state.anim.accordion_value(&key, is_pending);
        let header = button(
            row![
                chevron_icon(prog, pal.ink_secondary),
                text(format!("処理ログ · {} ステップ", act_rows.len()))
                    .size(12.5)
                    .font(UI_FONT_BOLD)
                    .color(pal.ink_secondary),
                space::horizontal(),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .padding([6, 8])
        .style(theme::btn_ghost(pal))
        .on_press(Message::ToggleAccordion(key.clone(), is_pending));

        let body_el: Element<'a, Message> = if prog > 0.015 {
            let items = column(
                act_rows
                    .iter()
                    .enumerate()
                    .map(|(sidx, r)| render_activity_row(state, jid, sidx, r, 0, pal)),
            )
            .spacing(6);
            let wrapped = container(items).padding(Padding { top: 8.0, right: 0.0, bottom: 2.0, left: 10.0 });
            if prog < 0.99 {
                container(wrapped)
                    .max_height((prog * 900.0).max(1.0))
                    .clip(true)
                    .into()
            } else {
                wrapped.into()
            }
        } else {
            space().into()
        };

        container(column![header, body_el])
            .padding([8, 12])
            .width(Length::Fill)
            .style(theme::surface2_style(pal, 12.0))
            .into()
    } else {
        space().into()
    };

    let final_ans = job
        .get("result")
        .and_then(|r| r.get("review"))
        .and_then(|rv| rv.get("final_answer"))
        .and_then(|v| v.as_str())
        .or_else(|| job.get("partialAnswer").and_then(|v| v.as_str()))
        .unwrap_or("");

    let ans_el: Element<'a, Message> = if !final_ans.is_empty() {
        if let Some((_, blocks)) = state.parsed_answers.get(jid) {
            markdown_view::render_markdown(
                blocks,
                pal,
                jid,
                state.copied_key.as_deref(),
                state.selection_scope.clone(),
                |k, c| Message::CopyText(c, Some(k)),
                Message::OpenExternal,
                |c| Message::SelectionCopied(c),
            )
        } else {
            SelectableText::new(final_ans.to_string())
                .font(UI_FONT)
                .size(14.0)
                .color(pal.ink)
                .selection_color(pal.accent.scale_alpha(0.28))
                .width(Length::Fill)
                .wrapping(Wrapping::WordOrGlyph)
                .scope(state.selection_scope.clone())
                .on_copy(|c| Message::SelectionCopied(c))
                .into()
        }
    } else if is_pending {
        row![
            thinking_dots(state.anim.pulse_phase, pal.accent),
            text(format!("{}…", status_jp(status)))
                .size(13.0)
                .color(pal.ink_muted),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
    } else if status == "waiting_user" {
        text("回答待ち")
            .size(13.0)
            .color(pal.accent)
            .into()
    } else {
        text(format!("状態: {}", status_jp(status)))
            .size(13.0)
            .color(pal.ink_muted)
            .into()
    };

    let head = row![
        text(model_badge(job)).size(12.0).font(UI_FONT_BOLD).color(pal.accent),
        space::horizontal(),
        if is_pending {
            Element::from(
                row![
                    pulse_dot(state.anim.pulse_phase, pal.accent, true),
                    text(status_jp(status)).size(12.0).color(pal.ink_secondary),
                ]
                .spacing(5)
                .align_y(Alignment::Center),
            )
        } else {
            text(status_jp(status)).size(12.0).color(pal.ink_muted).into()
        }
    ]
    .align_y(Alignment::Center);

    let mut card_col = column![head, act_group, ans_el].spacing(10);

    if !is_pending {
        let rc_line = job
            .get("_resultCard")
            .and_then(|r| r.get("line"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| if status == "completed" { "完了" } else { status_jp(status) });
        let rc_badge = job.get("_resultCard").and_then(|r| r.get("badge")).and_then(|v| v.as_str()).unwrap_or("");
        let badge_txt = if rc_badge.is_empty() {
            rc_line.to_string()
        } else {
            format!("{} · {}", rc_line, rc_badge)
        };
        let truncated = job.get("truncated").and_then(|v| v.as_bool()).unwrap_or(false);
        let ans_copy_key = format!("ans:{}", jid);
        let is_copied = state.copied_key.as_deref() == Some(ans_copy_key.as_str());

        let mut actions = row![].spacing(6);
        if truncated {
            actions = actions.push(
                button(text("省略分を読み込む").size(11.5))
                    .padding([3, 8])
                    .style(theme::btn_ghost(pal))
                    .on_press(Message::LoadFullJob(jid.to_string())),
            );
        }
        if !final_ans.is_empty() {
            actions = actions.push(
                button(text(if is_copied { "コピー済 ✓" } else { "回答をコピー" }).size(11.5))
                    .padding([3, 8])
                    .style(theme::btn_ghost(pal))
                    .on_press(Message::CopyText(final_ans.to_string(), Some(ans_copy_key))),
            );
        }
        actions = actions.push(
            button(
                row![
                    icon(IconKind::Download, 12.0, pal.ink_secondary),
                    text("保存").size(11.5).wrapping(Wrapping::None),
                ]
                .spacing(5)
                .align_y(Alignment::Center),
            )
            .padding([3, 8])
            .style(theme::btn_ghost(pal))
            .on_press(Message::SaveDownloads(Some(jid.to_string()))),
        );

        let metrics_txt = format_turn_metrics(job);
        let metrics_el: Element<'a, Message> = if !metrics_txt.is_empty() {
            text(metrics_txt)
                .size(11.5)
                .font(MONO_FONT)
                .color(pal.ink_secondary)
                .wrapping(Wrapping::None)
                .into()
        } else {
            space().width(0).height(0).into()
        };

        card_col = card_col.push(
            column![
                container(text(badge_txt).size(12.0).font(UI_FONT_BOLD).color(pal.amber))
                    .padding([5, 10])
                    .style(theme::box_style(pal.amber_soft, iced::Color::TRANSPARENT, 8.0, 0.0, Some(pal.amber), Default::default())),
                row![metrics_el, space::horizontal(), actions]
                    .spacing(8)
                    .align_y(Alignment::Center),
            ]
            .spacing(6),
        );
    }

    let card = container(card_col)
        .padding([16, 20])
        .width(Length::Fill)
        .style(theme::box_style(
            pal.bg_surface,
            if is_find_hi { pal.amber } else { pal.border_subtle },
            16.0,
            if is_find_hi { 2.0 } else { 1.0 },
            Some(pal.ink),
            pal.shadow_card,
        ));

    column![user_row, card].spacing(10).width(Length::Fill).into()
}

fn render_activity_row<'a>(
    state: &'a UiState,
    jid: &str,
    idx: usize,
    row_val: &'a Value,
    depth: usize,
    pal: UiPalette,
) -> Element<'a, Message> {
    let tool = row_val.get("tool").or_else(|| row_val.get("type")).and_then(|v| v.as_str()).unwrap_or("step");
    let row_status = row_val.get("status").and_then(|v| v.as_str()).unwrap_or("");
    let err = row_val.get("error").and_then(|v| v.as_str()).unwrap_or("");
    let exit_code = row_val.get("exitCode").and_then(|v| v.as_i64());
    let has_err = !err.is_empty() || exit_code.is_some_and(|c| c != 0);
    let think_txt = row_val
        .get("modelReasoning")
        .or_else(|| row_val.get("thinking"))
        .and_then(|v| v.as_str())
        .or_else(|| row_val.get("thoughtSummary").and_then(|v| v.as_str()))
        .or_else(|| if tool == "thinking" { row_val.get("text").and_then(|v| v.as_str()) } else { None })
        .unwrap_or("");
    let summary_head = row_val.get("thoughtSummary").and_then(|v| v.as_str()).unwrap_or("").trim();

    let icon = if tool == "thinking" || tool == "reasoning" {
        "💭"
    } else if tool.starts_with("local.run") {
        "⌘"
    } else if tool.starts_with("local.") {
        "📄"
    } else if tool.starts_with("desktop.") {
        "🖥"
    } else if tool == "delegate" || row_val.get("workerActivity").is_some() {
        "🤖"
    } else if has_err {
        "✕"
    } else {
        "✓"
    };

    let base_head = row_val
        .get("summary")
        .or_else(|| row_val.get("label"))
        .or_else(|| row_val.get("message"))
        .and_then(|v| v.as_str())
        .unwrap_or(tool);
    let headline = if tool == "thinking" || tool == "reasoning" {
        if !summary_head.is_empty() {
            summary_head.lines().next().unwrap_or("").chars().take(60).collect()
        } else if !think_txt.trim().is_empty() {
            if row_status == "running" {
                format!("思考中… ({} 文字)", think_txt.chars().count())
            } else {
                format!("思考ログ ({} 文字)", think_txt.chars().count())
            }
        } else if row_status == "running" {
            "思考を開始しています…".to_string()
        } else {
            "思考ログ".to_string()
        }
    } else if let Some(p) = row_val.get("path").and_then(|v| v.as_str()) {
        if !base_head.contains(p) {
            format!("{} · {}", base_head, p)
        } else {
            base_head.to_string()
        }
    } else if let Some(cmd) = row_val.get("command").and_then(|v| v.as_str()) {
        let first: String = cmd.trim().lines().next().unwrap_or("").chars().take(72).collect();
        format!("{}: {}", tool, first)
    } else {
        base_head.to_string()
    };

    let mut details: Vec<Element<'a, Message>> = Vec::new();
    if !err.is_empty() {
        details.push(
            SelectableText::new(err.to_string())
                .font(UI_FONT)
                .size(12.0)
                .color(pal.red)
                .selection_color(pal.accent.scale_alpha(0.28))
                .width(Length::Fill)
                .wrapping(Wrapping::WordOrGlyph)
                .scope(state.selection_scope.clone())
                .on_copy(|c| Message::SelectionCopied(c))
                .into(),
        );
    }
    if let Some(edit) = row_val.get("edit").and_then(|v| v.as_object()) {
        if edit.contains_key("before") || edit.contains_key("after") {
            let b = edit.get("before").and_then(|v| v.as_str()).unwrap_or("");
            let a = edit.get("after").and_then(|v| v.as_str()).unwrap_or("");
            details.push(markdown_view::render_diff_card(b, a, pal));
        }
    }
    if let Some(so) = row_val.get("stdout").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()) {
        let k = format!("{}:so:{}", jid, idx);
        details.push(markdown_view::render_code_card("stdout", so, k, pal, state.copied_key.as_deref(), state.selection_scope.clone(), |k, c| Message::CopyText(c, Some(k)), |c| Message::SelectionCopied(c)));
    }
    if let Some(se) = row_val.get("stderr").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()) {
        let k = format!("{}:se:{}", jid, idx);
        details.push(markdown_view::render_code_card("stderr", se, k, pal, state.copied_key.as_deref(), state.selection_scope.clone(), |k, c| Message::CopyText(c, Some(k)), |c| Message::SelectionCopied(c)));
    }
    if !think_txt.trim().is_empty() {
        details.push(
            SelectableText::new(think_txt.to_string())
                .font(UI_FONT)
                .size(12.5)
                .color(pal.ink_secondary)
                .selection_color(pal.accent.scale_alpha(0.28))
                .width(Length::Fill)
                .wrapping(Wrapping::WordOrGlyph)
                .scope(state.selection_scope.clone())
                .on_copy(|c| Message::SelectionCopied(c))
                .into(),
        );
    }
    let img_key = format!("actimg:{}:{}:{}", jid, depth, idx);
    if let Some(handle) = state.decoded_activity_imgs.get(&img_key) {
        details.push(
            button(
                img_w(handle.clone())
                    .width(Length::Fixed(320.0))
                    .height(Length::Fixed(180.0))
                    .content_fit(ContentFit::Contain),
            )
            .padding(4)
            .style(theme::btn_card(pal, false))
            .on_press(Message::OpenLightboxImage(headline.clone(), handle.clone()))
            .into(),
        );
    }
    if let Some(nested) = row_val.get("workerActivity").and_then(|v| v.as_array()) {
        if !nested.is_empty() && depth < 2 {
            let sub = column(nested.iter().enumerate().map(|(ni, sr)| {
                render_activity_row(state, jid, ni, sr, depth + 1, pal)
            }))
            .spacing(4);
            details.push(container(sub).padding(Padding { top: 4.0, right: 0.0, bottom: 0.0, left: 10.0 }).into());
        }
    }

    if details.is_empty() {
        return container(
            row![
                text(icon).size(12.5).color(if has_err { pal.red } else { pal.ink_secondary }),
                text(headline).size(12.5).color(if has_err { pal.red } else { pal.ink_secondary }),
            ]
            .spacing(7),
        )
        .padding([3, 6])
        .into();
    }

    let acc_key = format!("step:{}:{}:{}", jid, depth, idx);
    let default_open = depth == 0 && (has_err || ((tool == "thinking" || tool == "reasoning") && row_status == "running"));
    let prog = state.anim.accordion_value(&acc_key, default_open);
    let head_btn = button(
        row![
            chevron_icon(prog, pal.ink_muted),
            text(icon).size(12.5).color(if has_err { pal.red } else { pal.ink_secondary }),
            text(headline).size(12.5).color(if has_err { pal.red } else { pal.ink_secondary }).width(Length::Fill),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([4, 6])
    .style(theme::btn_ghost(pal))
    .on_press(Message::ToggleAccordion(acc_key, default_open));

    let body_el: Element<'a, Message> = if prog > 0.015 {
        let inner = container(column(details).spacing(8))
            .padding([8, 10])
            .width(Length::Fill)
            .style(theme::card_flat_style(pal));
        if prog < 0.99 {
            container(inner).max_height((prog * 640.0).max(1.0)).clip(true).into()
        } else {
            inner.into()
        }
    } else {
        space().into()
    };

    column![head_btn, body_el].spacing(4).into()
}

fn render_task_strip<'a>(state: &'a UiState, pal: UiPalette) -> Element<'a, Message> {
    if !state.anim.task_strip.is_visible() {
        return space().into();
    }
    let latest = state
        .conversation
        .as_ref()
        .and_then(|c| c.get("jobs"))
        .and_then(|a| a.as_array())
        .and_then(|arr| arr.last());
    let mut tasks = Vec::new();
    if let Some(j) = latest {
        let batches = j
            .get("result")
            .and_then(|r| r.get("batches"))
            .and_then(|b| b.as_array())
            .or_else(|| j.get("batches").and_then(|b| b.as_array()));
        if let Some(bs) = batches {
            for b in bs {
                if let Some(ts) = b.get("tasks").and_then(|t| t.as_array()) {
                    tasks.extend(ts.iter());
                }
            }
        }
    }
    if tasks.is_empty() {
        return space().into();
    }
    let done = tasks.iter().filter(|t| t.get("status").and_then(|v| v.as_str()) == Some("completed")).count();
    let body_v = state.anim.task_body.value();
    let head = button(
        row![
            chevron_icon(body_v, pal.ink_secondary),
            text(format!("8B 並列タスク進捗 ({} / {})", done, tasks.len()))
                .size(12.5)
                .font(UI_FONT_BOLD),
            space::horizontal(),
            text(if state.task_strip_collapsed { "展開" } else { "折りたたむ" })
                .size(11.5)
                .color(pal.ink_muted),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([7, 14])
    .style(theme::btn_ghost(pal))
    .on_press(Message::ToggleTaskCollapse);

    let list_el: Element<'a, Message> = if body_v > 0.015 {
        let rows = column(tasks.into_iter().map(|t| {
            let st = t.get("status").and_then(|v| v.as_str()).unwrap_or("queued");
            let mark = match st {
                "completed" => "✓",
                "running" => "●",
                "failed" => "✕",
                _ => "○",
            };
            let lbl = t
                .get("title")
                .or_else(|| t.get("name"))
                .or_else(|| t.get("goal"))
                .and_then(|v| v.as_str())
                .unwrap_or("task");
            row![
                text(format!("{} {}", mark, lbl)).size(12.0).width(Length::Fill),
                text(status_jp(st)).size(12.0).color(pal.ink_muted),
            ]
            .into()
        }))
        .spacing(3);
        container(
            scrollable(container(rows).padding([0, 14]))
                .height(Length::Fixed((110.0 * body_v).max(1.0)))
                .style(theme::scrollable_style(pal)),
        )
        .clip(true)
        .into()
    } else {
        space().into()
    };

    container(
        container(column![head, list_el])
            .max_width(868)
            .width(Length::Fill)
            .style(theme::card_flat_style(pal)),
    )
    .center_x(Length::Fill)
    .padding([0, 18])
    .into()
}

fn render_question_panel<'a>(state: &'a UiState, pal: UiPalette) -> Element<'a, Message> {
    if !state.anim.question.is_visible() {
        return space().into();
    }
    let q = state
        .conversation
        .as_ref()
        .and_then(|c| c.get("jobs"))
        .and_then(|a| a.as_array())
        .and_then(|arr| arr.last())
        .filter(|j| j.get("status").and_then(|v| v.as_str()) == Some("waiting_user"))
        .and_then(|j| j.get("question"));
    let Some(q_obj) = q else {
        return space().into();
    };
    let qid = q_obj.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let prompt_t = q_obj
        .get("prompt")
        .or_else(|| q_obj.get("question"))
        .or_else(|| q_obj.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let opts = q_obj
        .get("options")
        .or_else(|| q_obj.get("choices"))
        .and_then(|v| v.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[]);

    let mut col = column![
        text("回答を選択")
            .size(12.5)
            .font(UI_FONT_BOLD)
            .color(pal.accent),
    ]
    .spacing(8);
    if !prompt_t.is_empty() {
        col = col.push(text(prompt_t.to_string()).size(13.0).font(UI_FONT_BOLD));
    }
    for opt in opts {
        let lbl = opt
            .as_str()
            .or_else(|| opt.get("label").and_then(|v| v.as_str()))
            .or_else(|| opt.get("text").and_then(|v| v.as_str()))
            .unwrap_or("");
        let val = opt
            .as_str()
            .or_else(|| opt.get("value").and_then(|v| v.as_str()))
            .or_else(|| opt.get("label").and_then(|v| v.as_str()))
            .unwrap_or(lbl)
            .to_string();
        col = col.push(
            button(text(lbl.to_string()).size(12.5))
                .width(Length::Fill)
                .padding([7, 12])
                .style(theme::btn_secondary(pal))
                .on_press_maybe((!state.read_only).then(|| Message::AnswerQuestion(qid.clone(), val))),
        );
    }

    let v = state.anim.question.value();
    container(
        container(col)
            .max_width(868)
            .width(Length::Fill)
            .padding([12, 14])
            .style(theme::box_style(
                pal.accent_soft.scale_alpha(v),
                pal.accent_border.scale_alpha(v),
                14.0,
                1.0,
                Some(pal.ink),
                Default::default(),
            )),
    )
    .center_x(Length::Fill)
    .padding([4, 18])
    .into()
}

fn render_notice_banner<'a>(state: &'a UiState, pal: UiPalette) -> Element<'a, Message> {
    if !state.anim.notice.is_visible() {
        return space().into();
    }
    let Some(ref n) = state.notice else {
        return space().into();
    };
    let v = state.anim.notice.value();
    let lvl = n.get("level").and_then(|x| x.as_str()).unwrap_or("warn");
    let (bg, border_c) = match lvl {
        "error" => (pal.red_soft, pal.red.scale_alpha(0.42)),
        "info" => (pal.accent_soft, pal.accent_border),
        _ => (pal.amber_soft, pal.amber.scale_alpha(0.42)),
    };
    let label = n.get("label").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let hint = n.get("hint").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let raw = n.get("raw").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let retry = n.get("retry").and_then(|x| x.as_bool()).unwrap_or(false);

    let mut left = column![text(label).size(13.0).font(UI_FONT_BOLD)].spacing(3);
    if !hint.is_empty() {
        left = left.push(text(hint).size(12.0).color(pal.ink_secondary));
    }
    if !raw.is_empty() {
        left = left.push(
            container(text(raw).size(11.5).font(MONO_FONT).color(pal.ink_secondary))
                .padding([5, 8])
                .width(Length::Fill)
                .style(theme::surface2_style(pal, 8.0)),
        );
    }

    container(
        container(
            row![
                left.width(Length::Fill),
                if retry {
                    Element::from(
                        button(text("続ける").size(12.0))
                            .padding([5, 11])
                            .style(theme::btn_primary(pal))
                            .on_press(Message::ContinueChat),
                    )
                } else {
                    space().into()
                },
                icon_button(IconKind::Close, 26.0, 26.0, 12.0, pal, Message::CloseNotice),
            ]
            .spacing(8)
            .align_y(Alignment::Start),
        )
        .max_width(868)
        .width(Length::Fill)
        .padding([10, 14])
        .style(theme::box_style(
            bg.scale_alpha(v),
            border_c.scale_alpha(v),
            14.0,
            1.0,
            Some(pal.ink),
            Default::default(),
        )),
    )
    .center_x(Length::Fill)
    .padding([4, 18])
    .into()
}

fn render_attach_tray<'a>(state: &'a UiState, pal: UiPalette) -> Element<'a, Message> {
    if !state.anim.attach_tray.is_visible() || state.attachments.is_empty() {
        return space().into();
    }
    let cards = scrollable(
        row(state.attachments.iter().enumerate().map(|(idx, a)| {
            let p = a.get("path").and_then(|v| v.as_str()).unwrap_or("file").to_string();
            let display = a
                .get("displayName")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| p.strip_prefix("uploads/").unwrap_or(&p))
                .to_string();
            let kb = (a.get("size").and_then(|v| v.as_u64()).unwrap_or(0) / 1024).max(1);
            let st = state.upload_stages.get(&p).and_then(|v| v.as_str()).unwrap_or("");
            let sub = match st {
                "uploading" => format!("{} KB · 送信準備中", kb),
                "uploaded" => format!("{} KB · 待機済", kb),
                "failed" => format!("{} KB · 送信時再送", kb),
                _ => format!("{} KB", kb),
            };
            let thumb: Element<'a, Message> = if let Some(h) = state.attachment_thumbs.get(&p) {
                img_w(h.clone())
                    .width(34)
                    .height(34)
                    .content_fit(ContentFit::Cover)
                    .into()
            } else {
                container(icon(IconKind::Paperclip, 15.0, pal.ink_secondary))
                    .center(34)
                    .style(theme::surface2_style(pal, 6.0))
                    .into()
            };
            let p_del = p.clone();
            button(
                row![
                    thumb,
                    column![
                        container(text(display).size(12.0).font(UI_FONT_BOLD).wrapping(Wrapping::None))
                            .width(Length::Fill)
                            .clip(true),
                        text(sub).size(10.5).color(pal.ink_muted).wrapping(Wrapping::None),
                    ]
                    .width(Length::Fill),
                    icon_button(IconKind::Close, 22.0, 22.0, 10.5, pal, Message::RemoveAttachment(p_del)),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .width(Length::Fixed(208.0))
            .padding([4, 8])
            .style(theme::btn_card(pal, false))
            .on_press(Message::PreviewAttachment(idx))
            .into()
        }))
        .spacing(8),
    )
    .horizontal()
    .width(Length::Fill)
    .style(theme::scrollable_style(pal));

    container(
        container(
            row![
                cards,
                button(text("全解除").size(11.5).wrapping(Wrapping::None))
                    .padding([4, 9])
                    .style(theme::btn_ghost(pal))
                    .on_press(Message::ClearAttachments),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .max_width(868)
        .width(Length::Fill)
        .padding([8, 12])
        .style(theme::card_flat_style(pal)),
    )
    .center_x(Length::Fill)
    .padding([4, 18])
    .into()
}

fn render_composer<'a>(state: &'a UiState, pal: UiPalette) -> Element<'a, Message> {
    let prompt_txt = state.composer_content.text();
    let slash_hits = filtered_slash_items(&prompt_txt);
    let slash_vis = state.anim.slash_menu.is_visible() && !slash_hits.is_empty();

    let slash_el: Element<'a, Message> = if slash_vis {
        let sel = state.slash_idx.min(slash_hits.len().saturating_sub(1));
        let list_col = column(slash_hits.into_iter().enumerate().map(|(i, it)| {
            let selected = !it.disabled && i == sel;
            let lbl_color = if it.disabled {
                pal.ink_muted.scale_alpha(0.50)
            } else if selected {
                pal.accent
            } else {
                pal.ink
            };
            let desc_color = if it.disabled {
                pal.ink_muted.scale_alpha(0.45)
            } else {
                pal.ink_muted
            };
            button(
                row![
                    text(it.label).size(12.5).font(UI_FONT_BOLD).color(lbl_color),
                    space::horizontal(),
                    text(it.desc).size(11.5).color(desc_color),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([6, 10])
            .style(theme::btn_history_row(pal, selected))
            .on_press_maybe((!it.disabled).then_some(Message::SlashPick(i)))
            .into()
        }))
        .spacing(3);
        container(
            scrollable(list_col)
                .height(Length::Shrink)
                .style(theme::scrollable_style(pal)),
        )
        .max_width(480)
        .max_height(260.0)
        .padding(6)
        .style(theme::box_style(pal.bg_surface, pal.border_strong, 12.0, 1.0, Some(pal.ink), pal.shadow_float))
        .into()
    } else {
        space().into()
    };

    let slash_open_now = !filtered_slash_items(&prompt_txt).is_empty();
    let editor = text_editor(&state.composer_content)
        .placeholder("メッセージを入力…")
        .height(Length::Shrink)
        .min_height(46.0)
        .max_height(175.0)
        .padding([4, 6])
        .on_action(Message::EditorAction)
        .style(theme::editor_transparent_style(pal))
        .key_binding(move |kp| {
            if kp.modifiers.control() && matches!(kp.key.as_ref(), keyboard::Key::Named(keyboard::key::Named::Enter)) {
                return Some(text_editor::Binding::Custom(Message::SendPrompt));
            }
            if kp.modifiers.control() && kp.key.to_latin(kp.physical_key) == Some('v') && !kp.modifiers.alt() {
                return Some(text_editor::Binding::Custom(Message::ComposerCtrlV));
            }
            if slash_open_now {
                match kp.modified_key.as_ref() {
                    keyboard::Key::Named(keyboard::key::Named::ArrowDown) => {
                        return Some(text_editor::Binding::Custom(Message::SlashMove(1)));
                    }
                    keyboard::Key::Named(keyboard::key::Named::ArrowUp) => {
                        return Some(text_editor::Binding::Custom(Message::SlashMove(-1)));
                    }
                    keyboard::Key::Named(keyboard::key::Named::Tab) => {
                        return Some(text_editor::Binding::Custom(Message::SlashAccept));
                    }
                    keyboard::Key::Named(keyboard::key::Named::Enter)
                        if !kp.modifiers.shift() && !kp.modifiers.control() =>
                    {
                        return Some(text_editor::Binding::Custom(Message::SlashAccept));
                    }
                    _ => {}
                }
            }
            text_editor::Binding::from_key_press(kp)
        });

    let mode_opt = WorkModeOpt::from_str(state.gui.get("workMode").and_then(|v| v.as_str()).unwrap_or("standard"));
    let model_seg = container(
        row![
            container(
                text("27B")
                    .size(12.0)
                    .font(UI_FONT_BOLD)
                    .color(pal.accent)
                    .wrapping(Wrapping::None),
            )
            .padding([3, 9])
            .style(theme::box_style(
                pal.accent_soft,
                pal.accent_border,
                7.0,
                1.0,
                Some(pal.accent),
                Default::default(),
            )),
            button(
                text("8B")
                    .size(12.0)
                    .font(UI_FONT_BOLD)
                    .wrapping(Wrapping::None),
            )
            .padding([3, 8])
            .style(theme::btn_tab(pal, false)),
        ]
        .spacing(2)
        .align_y(Alignment::Center),
    )
    .padding(2)
    .style(theme::box_style(
        pal.bg_surface_2.scale_alpha(0.65),
        pal.border_subtle,
        9.0,
        1.0,
        None,
        Default::default(),
    ));

    let ctx = state
        .server
        .get("contextWindow")
        .or_else(|| state.gui.get("contextWindow"))
        .and_then(|v| v.as_i64())
        .unwrap_or(16384);
    let tps = state
        .perf
        .get("tokensPerSec")
        .or_else(|| state.perf.get("lastTokensPerSec"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let sum_lbl = if tps > 0.0 {
        format!("Qwen27 / {}k · {:.1}tok/s", ctx / 1000, tps)
    } else {
        format!("Qwen27 / {}k", ctx / 1000)
    };

    let waiting_user = state
        .conversation
        .as_ref()
        .and_then(|c| c.get("jobs"))
        .and_then(|a| a.as_array())
        .is_some_and(|arr| arr.iter().any(|j| j.get("status").and_then(|v| v.as_str()) == Some("waiting_user")));

    let char_len = prompt_txt.chars().count();
    let left_controls = row![
        model_seg,
        pick_list(WorkModeOpt::ALL, Some(mode_opt), Message::SelectWorkMode)
            .text_size(12.0)
            .padding([5, 9])
            .style(theme::pick_list_style(pal))
            .menu_style(theme::pick_menu_style(pal)),
        button(text("Direct 27B").size(12.0).font(UI_FONT_BOLD).wrapping(Wrapping::None))
            .padding([5, 10])
            .style(theme::btn_chip(pal, false)),
        button(text(sum_lbl).size(12.0).wrapping(Wrapping::None))
            .padding([5, 10])
            .style(theme::btn_chip(pal, true))
            .on_press(Message::OpenTuningQuick),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .wrap();

    let mut right_items: Vec<Element<'a, Message>> = vec![
        text(format!("{}/6000", char_len))
            .size(11.0)
            .color(pal.ink_muted)
            .wrapping(Wrapping::None)
            .into(),
        button(
            row![
                icon(IconKind::Paperclip, 13.0, pal.ink_secondary),
                text("添付").size(12.5).wrapping(Wrapping::None),
            ]
            .spacing(5)
            .align_y(Alignment::Center),
        )
        .padding([5, 10])
        .style(theme::btn_ghost(pal))
        .on_press(Message::PickAttachments)
        .into(),
    ];
    if state.busy || waiting_user {
        right_items.push(
            button(text("中断").size(12.5).wrapping(Wrapping::None))
                .padding([5, 11])
                .style(theme::btn_danger(pal))
                .on_press(Message::CancelJob)
                .into(),
        );
    }
    right_items.push(
        button(
            row![
                text("送信").size(12.5).font(UI_FONT_BOLD).wrapping(Wrapping::None),
                icon(IconKind::Send, 13.0, iced::Color::WHITE),
            ]
            .spacing(5)
            .align_y(Alignment::Center),
        )
        .padding([6, 13])
        .style(theme::btn_primary(pal))
        .on_press_maybe((!state.read_only && !state.busy).then_some(Message::SendPrompt))
        .into(),
    );
    let right_controls = row(right_items).spacing(6).align_y(Alignment::Center);
    let controls = row![
        container(left_controls).width(Length::Fill),
        right_controls,
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let box_card = container(
        column![
            editor,
            iced::widget::rule::horizontal(1).style(theme::rule_subtle(pal)),
            controls,
        ]
        .spacing(6),
    )
    .padding([10, 12])
    .width(Length::Fill)
    .style(theme::composer_box_style(pal, true));

    container(
        column![slash_el, box_card]
            .spacing(8)
            .max_width(868)
            .width(Length::Fill),
    )
    .center_x(Length::Fill)
    .padding(Padding { top: 4.0, right: 18.0, bottom: 12.0, left: 18.0 })
    .into()
}
