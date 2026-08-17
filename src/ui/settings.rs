//! Settings window: credentials, models, limits, appearance, about.

use crate::app::{App, CredentialState, KeyTestStatus, Screen, SettingsTab};
use crate::secure_store::SecureStore;
use crate::storage::{self, MotionPreference, ThemePreference};
use eframe::egui;

pub fn render(app: &mut App, ui: &mut egui::Ui) {
    let (open, confirm_remove) = match &app.screen {
        Screen::Main(state) => (state.settings_ui.open, state.settings_ui.confirm_remove),
        _ => return,
    };
    if !open {
        return;
    }

    let mut still_open = true;
    egui::Window::new("Settings")
        .open(&mut still_open)
        .resizable(true)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .default_width(620.0)
        .default_height(440.0)
        .frame(crate::ui::theme::panel(ui))
        .show(ui.ctx(), |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(140.0);
                    tab_button(ui, app, SettingsTab::Credentials, "OpenRouter");
                    tab_button(ui, app, SettingsTab::Anthropic, "Anthropic");
                    tab_button(ui, app, SettingsTab::Local, "Local");
                    tab_button(ui, app, SettingsTab::Models, "Models");
                    tab_button(ui, app, SettingsTab::Limits, "Limits");
                    tab_button(ui, app, SettingsTab::Appearance, "Appearance");
                    tab_button(ui, app, SettingsTab::Shortcuts, "Shortcuts");
                    tab_button(ui, app, SettingsTab::Mcp, "MCP");
                    tab_button(ui, app, SettingsTab::About, "About");
                });
                ui.separator();
                ui.vertical(|ui| match current_tab(app) {
                    SettingsTab::Credentials => render_credentials(app, ui),
                    SettingsTab::Anthropic => render_anthropic(app, ui),
                    SettingsTab::Local => render_local(app, ui),
                    SettingsTab::Models => render_models(app, ui),
                    SettingsTab::Limits => render_limits(app, ui),
                    SettingsTab::Appearance => render_appearance(app, ui),
                    SettingsTab::Shortcuts => render_shortcuts(ui),
                    SettingsTab::Mcp => render_mcp(app, ui),
                    SettingsTab::About => render_about(app, ui),
                });
            });
        });

    if !still_open {
        app.close_settings();
    }

    if confirm_remove {
        render_remove_confirm(app, ui);
    }
}

fn current_tab(app: &App) -> SettingsTab {
    match &app.screen {
        Screen::Main(state) => state.settings_ui.tab,
        _ => SettingsTab::Credentials,
    }
}

fn tab_button(ui: &mut egui::Ui, app: &mut App, tab: SettingsTab, label: &str) {
    let selected = current_tab(app) == tab;
    if ui.selectable_label(selected, label).clicked()
        && let Screen::Main(state) = &mut app.screen
    {
        state.settings_ui.tab = tab;
    }
}

fn render_credentials(app: &mut App, ui: &mut egui::Ui) {
    let (state, masked, testing) = match &app.screen {
        Screen::Main(s) => (
            s.credential.state,
            s.credential.masked.clone(),
            matches!(s.settings_ui.test_status, KeyTestStatus::Testing),
        ),
        _ => return,
    };

    ui.heading("Credentials");
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label("API key");
        match state {
            CredentialState::Present => {
                ui.colored_label(crate::ui::theme::tokens(ui).success, "✓ Configured");
            }
            CredentialState::Missing => {
                ui.colored_label(crate::ui::theme::tokens(ui).danger, "✗ Missing");
            }
            CredentialState::Rejected => {
                ui.colored_label(crate::ui::theme::tokens(ui).warning, "⚠ Rejected");
            }
        }
    });

    if !masked.is_empty() {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(masked)
                .monospace()
                .color(crate::ui::theme::tokens(ui).text_muted),
        );
    } else {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("No key stored.")
                .italics()
                .color(crate::ui::theme::tokens(ui).text_muted),
        );
    }

    ui.add_space(12.0);
    ui.label("Insert or replace");
    ui.add_space(4.0);

    match &mut app.screen {
        Screen::Main(state) => {
            ui.add(
                egui::TextEdit::singleline(&mut state.settings_ui.key_input)
                    .password(!state.settings_ui.show_key)
                    .hint_text("sk-or-v1-…")
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(4.0);
            ui.checkbox(&mut state.settings_ui.show_key, "Show typed key");
        }
        _ => return,
    }

    ui.add_space(10.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !testing,
                egui::Button::new(if testing { "Testing…" } else { "Test" }),
            )
            .on_hover_text("Validate the typed key, or the stored key if the field is empty")
            .clicked()
        {
            app.start_key_test();
        }
        if ui
            .add_enabled(
                state != CredentialState::Missing,
                egui::Button::new("Remove"),
            )
            .clicked()
            && let Screen::Main(s) = &mut app.screen
        {
            s.settings_ui.confirm_remove = true;
        }
    });

    if let Screen::Main(state) = &app.screen {
        match &state.settings_ui.test_status {
            KeyTestStatus::Idle | KeyTestStatus::Testing => {}
            KeyTestStatus::Ok => {
                ui.add_space(8.0);
                ui.colored_label(crate::ui::theme::tokens(ui).success, "Key accepted.");
            }
            KeyTestStatus::Err(msg) => {
                ui.add_space(8.0);
                ui.colored_label(crate::ui::theme::tokens(ui).danger, msg);
            }
        }
    }

    ui.add_space(12.0);
    ui.label(
        egui::RichText::new(format!(
            "The key is stored in {} and is never shown in full.",
            SecureStore::display_name()
        ))
        .small()
        .color(crate::ui::theme::tokens(ui).text_muted),
    );
}

fn render_anthropic(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Anthropic");
    ui.add_space(8.0);
    ui.label(
        "Direct Anthropic API key. Used when you pick a Claude model from the Anthropic group.",
    );
    ui.add_space(10.0);
    render_provider_key_field(app, ui, crate::providers::ANTHROPIC, "sk-ant-…");
}

fn render_local(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Local / OpenAI-compatible");
    ui.add_space(8.0);
    ui.label(
        "Ollama, LM Studio, vLLM or LiteLLM. Models are discovered from /v1/models and billed at $0.00.",
    );
    ui.add_space(10.0);
    let Screen::Main(state) = &mut app.screen else {
        return;
    };
    ui.label("Base URL");
    let changed = ui
        .add(
            egui::TextEdit::singleline(&mut state.settings.openai_compat_base_url)
                .hint_text(crate::providers::openai_compat::DEFAULT_LOCAL_BASE_URL)
                .desired_width(f32::INFINITY),
        )
        .changed();
    if changed {
        let _ = crate::storage::save_settings(&state.settings);
    }
    ui.add_space(10.0);
    render_provider_key_field(app, ui, crate::providers::OPENAI_COMPAT, "optional API key");
}

fn render_provider_key_field(app: &mut App, ui: &mut egui::Ui, provider: &'static str, hint: &str) {
    let testing = matches!(
        match &app.screen {
            Screen::Main(s) => &s.settings_ui.test_status,
            _ => return,
        },
        KeyTestStatus::Testing
    );
    match &mut app.screen {
        Screen::Main(state) => {
            ui.add(
                egui::TextEdit::singleline(&mut state.settings_ui.key_input)
                    .password(!state.settings_ui.show_key)
                    .hint_text(hint)
                    .desired_width(f32::INFINITY),
            );
            ui.checkbox(&mut state.settings_ui.show_key, "Show typed key");
        }
        _ => return,
    }
    ui.add_space(8.0);
    if ui
        .add_enabled(
            !testing,
            egui::Button::new(if testing { "Testing…" } else { "Test" }),
        )
        .clicked()
    {
        let key = match &app.screen {
            Screen::Main(s) => s.settings_ui.key_input.trim().to_string(),
            _ => String::new(),
        };
        let key = if key.is_empty() {
            crate::secure_store::SecureStore::load_key_for(provider)
                .ok()
                .flatten()
                .unwrap_or_default()
        } else {
            key
        };
        app.start_provider_key_test(provider, key);
    }
    if let Screen::Main(state) = &app.screen {
        match &state.settings_ui.test_status {
            KeyTestStatus::Ok => {
                ui.colored_label(crate::ui::theme::tokens(ui).success, "Accepted.");
            }
            KeyTestStatus::Err(msg) => {
                ui.colored_label(crate::ui::theme::tokens(ui).danger, msg);
            }
            _ => {}
        }
    }
}

fn render_models(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Models");
    ui.add_space(8.0);
    ui.label("Default model for new Chat Mode conversations and new Coder sessions.");
    ui.add_space(12.0);

    let Screen::Main(state) = &mut app.screen else {
        return;
    };
    let mut dirty = false;

    ui.label("Chat Mode");
    dirty |= model_combo(
        ui,
        "settings_chat_model",
        &mut state.settings.chat_default_model,
        &state.catalog,
    );
    ui.add_space(10.0);
    ui.label("Coder Mode");
    dirty |= model_combo(
        ui,
        "settings_coder_model",
        &mut state.settings.coder_default_model,
        &state.catalog,
    );

    if dirty {
        let _ = storage::save_settings(&state.settings);
    }
}

fn model_combo(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut String,
    catalog: &crate::providers::catalog::ModelCatalog,
) -> bool {
    let mut changed = false;
    let label = catalog.display_name(value);
    egui::ComboBox::from_id_salt(id)
        .selected_text(label)
        .width(360.0)
        .show_ui(ui, |ui| {
            let refs: Vec<_> = catalog
                .highlights
                .iter()
                .chain(catalog.all.iter())
                .collect();
            let mut seen = std::collections::HashSet::new();
            let unique: Vec<_> = refs
                .into_iter()
                .filter(|m| seen.insert(m.id.clone()))
                .collect();
            for (provider_id, models) in crate::providers::catalog::group_by_provider(&unique) {
                ui.label(
                    egui::RichText::new(
                        crate::providers::catalog::provider_label(&provider_id).to_uppercase(),
                    )
                    .small()
                    .strong()
                    .color(crate::ui::theme::tokens(ui).text_muted),
                );
                for model in models {
                    if ui
                        .selectable_label(*value == model.id, &model.name)
                        .clicked()
                    {
                        *value = model.id;
                        changed = true;
                    }
                }
            }
        });
    changed
}

fn render_limits(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Limits");
    ui.add_space(8.0);
    let mut dirty = false;
    let mut rebuild_timeout = None;
    {
        let Screen::Main(state) = &mut app.screen else {
            return;
        };

        ui.horizontal(|ui| {
            ui.label("Spend cap per session (USD)");
            let mut value = state.settings.session_budget_usd;
            if ui
                .add(
                    egui::DragValue::new(&mut value)
                        .range(0.1..=100.0)
                        .speed(0.1)
                        .prefix("$"),
                )
                .changed()
            {
                state.settings.session_budget_usd = value;
                dirty = true;
            }
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Maximum agent iterations");
            let mut value = state.settings.max_iterations;
            if ui
                .add(egui::DragValue::new(&mut value).range(1..=200).speed(1.0))
                .changed()
            {
                state.settings.max_iterations = value;
                dirty = true;
            }
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Request timeout (seconds)");
            let mut value = state.settings.request_timeout_secs;
            if ui
                .add(egui::DragValue::new(&mut value).range(15..=600).speed(1.0))
                .changed()
            {
                state.settings.request_timeout_secs = value;
                dirty = true;
            }
        });
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(
                "New sessions pick up these limits. Changing the timeout rebuilds the API client.",
            )
            .small()
            .color(crate::ui::theme::tokens(ui).text_muted),
        );

        if dirty {
            let _ = storage::save_settings(&state.settings);
            rebuild_timeout = Some(state.settings.request_timeout_secs);
        }
    }
    if let Some(timeout) = rebuild_timeout {
        app.rebuild_provider_timeout(timeout);
    }
}

fn render_appearance(app: &mut App, ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new("APPEARANCE // CONSOLE PROFILE")
            .strong()
            .monospace()
            .color(crate::ui::theme::tokens(ui).text_primary),
    );
    ui.add_space(8.0);
    let mut dirty = false;
    if let Screen::Main(state) = &mut app.screen {
        ui.horizontal(|ui| {
            ui.label("Theme");
            let label = match state.settings.theme {
                ThemePreference::System => "Follow system",
                ThemePreference::Light => "Light",
                ThemePreference::Dark => "Dark",
            };
            egui::ComboBox::from_id_salt("settings_theme")
                .selected_text(label)
                .show_ui(ui, |ui| {
                    dirty |= ui
                        .selectable_value(
                            &mut state.settings.theme,
                            ThemePreference::System,
                            "Follow system",
                        )
                        .changed();
                    dirty |= ui
                        .selectable_value(
                            &mut state.settings.theme,
                            ThemePreference::Light,
                            "Light",
                        )
                        .changed();
                    dirty |= ui
                        .selectable_value(&mut state.settings.theme, ThemePreference::Dark, "Dark")
                        .changed();
                });
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Motion");
            let motion_label = match state.settings.motion {
                MotionPreference::Full => "Full feedback",
                MotionPreference::Reduced => "Reduced",
            };
            egui::ComboBox::from_id_salt("settings_motion")
                .selected_text(motion_label)
                .show_ui(ui, |ui| {
                    dirty |= ui
                        .selectable_value(
                            &mut state.settings.motion,
                            MotionPreference::Full,
                            "Full feedback",
                        )
                        .changed();
                    dirty |= ui
                        .selectable_value(
                            &mut state.settings.motion,
                            MotionPreference::Reduced,
                            "Reduced",
                        )
                        .changed();
                });
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Font scale");
            let mut percent = state.settings.font_scale * 100.0;
            if ui
                .add(egui::Slider::new(&mut percent, 80.0..=200.0).suffix("%"))
                .changed()
            {
                state.settings.font_scale =
                    (percent / 100.0).clamp(storage::MIN_FONT_SCALE, storage::MAX_FONT_SCALE);
                dirty = true;
            }
        });
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("Ctrl + / Ctrl - / Ctrl 0 also change the scale.")
                .small()
                .color(crate::ui::theme::tokens(ui).text_muted),
        );
    }
    if dirty {
        app.persist_theme_settings();
    }
}

fn render_shortcuts(ui: &mut egui::Ui) {
    ui.heading("Keyboard shortcuts");
    ui.add_space(8.0);
    egui::Grid::new("shortcuts_table")
        .num_columns(2)
        .spacing([24.0, 6.0])
        .show(ui, |ui| {
            ui.label(egui::RichText::new("Action").strong());
            ui.label(egui::RichText::new("Keys").strong());
            ui.end_row();
            for shortcut in crate::ui::shortcuts::SHORTCUTS {
                ui.label(shortcut.label);
                ui.monospace(shortcut.keys);
                ui.end_row();
            }
        });
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(
            "Modifier shortcuts work while typing. Esc cancels a confirm, closes Settings, then stops generation.",
        )
        .small()
        .color(crate::ui::theme::tokens(ui).text_muted),
    );
}

fn render_mcp(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("MCP servers");
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(
            "Declared in .orbit/config.toml. The first run on this machine needs explicit trust. \
             Risk overrides stay on this computer.",
        )
        .small()
        .color(crate::ui::theme::tokens(ui).text_muted),
    );
    ui.add_space(8.0);
    let Screen::Main(state) = &app.screen else {
        return;
    };
    if state.coder.project.is_none() {
        ui.label("Open a project to load MCP servers.");
        return;
    }
    let Ok(mgr) = state.coder.mcp.lock() else {
        ui.label("MCP lock busy");
        return;
    };
    if mgr.servers.is_empty() {
        ui.label("No [[mcp.servers]] entries in .orbit/config.toml.");
        return;
    }
    let mut trust = None;
    let mut toggle_risk: Option<(String, crate::tools::ToolRisk)> = None;
    for server in &mgr.servers {
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.strong(&server.config.name);
                ui.label(match &server.status {
                    crate::mcp::ServerStatus::Running => "running",
                    crate::mcp::ServerStatus::Stopped => "stopped",
                    crate::mcp::ServerStatus::NeedsTrust => "needs trust",
                    crate::mcp::ServerStatus::Denied => "denied",
                    crate::mcp::ServerStatus::Failed(e) => e.as_str(),
                });
            });
            ui.monospace(server.config.display());
            if server.status == crate::mcp::ServerStatus::NeedsTrust
                && ui.button("Trust and start").clicked()
            {
                trust = Some(server.config.name.clone());
            }
            for tool in &server.tools {
                let qualified = crate::mcp::tool::qualified_name(&server.config.name, &tool.name);
                let risk = server.tool_risk(&tool.name);
                ui.horizontal(|ui| {
                    ui.label(&qualified);
                    ui.label(format!("{risk:?}"));
                    if risk != crate::tools::ToolRisk::ReadOnly
                        && ui.small_button("Mark read-only").clicked()
                    {
                        toggle_risk = Some((qualified.clone(), crate::tools::ToolRisk::ReadOnly));
                    }
                    if risk == crate::tools::ToolRisk::ReadOnly
                        && ui.small_button("Mark executing").clicked()
                    {
                        toggle_risk = Some((qualified, crate::tools::ToolRisk::Executing));
                    }
                });
            }
        });
        ui.add_space(6.0);
    }
    drop(mgr);
    if let Some(name) = trust {
        app.trust_mcp_server(&name);
    }
    if let Some((qualified, risk)) = toggle_risk {
        let _ = crate::mcp::trust::set_risk_override(&qualified, risk);
    }
}

fn render_about(app: &mut App, ui: &mut egui::Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(4.0);
        ui.heading("Orbit");
        ui.label(
            egui::RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                .color(crate::ui::theme::tokens(ui).text_muted),
        );
    });
    ui.add_space(10.0);
    ui.separator();
    ui.add_space(10.0);
    ui.label("Orbit");
    ui.add_space(8.0);
    ui.label("Developed by: Lumen Connection");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("Contact:");
        ui.hyperlink_to("Website/Portfolio", "https://lumenconnection.com.br");
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("Source:");
        ui.hyperlink_to("GitHub", "https://github.com/Lumen-Connection/Orbit");
    });
    ui.add_space(12.0);
    ui.label(
        egui::RichText::new(format!(
            "Your API key is stored in {}.\n{}",
            SecureStore::display_name(),
            storage::chat_history_location_description()
        ))
        .small()
        .color(crate::ui::theme::tokens(ui).text_muted),
    );
    ui.add_space(8.0);
    if ui.button("Export diagnostics…").clicked() {
        app.export_diagnostics();
    }
}

fn render_remove_confirm(app: &mut App, ui: &mut egui::Ui) {
    let mut open = true;
    let mut confirm = false;
    let mut cancel = false;
    egui::Window::new("Remove API key?")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .default_width(360.0)
        .show(ui.ctx(), |ui| {
            ui.label("The stored key will be deleted. Saved chats stay on disk.");
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Remove").clicked() {
                    confirm = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    if confirm {
        app.remove_api_key();
    } else if (cancel || !open)
        && let Screen::Main(state) = &mut app.screen
    {
        state.settings_ui.confirm_remove = false;
    }
}
