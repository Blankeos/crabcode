use crate::command::parser::ParsedCommand;
use crate::command::registry::{Command, CommandResult, Registry};
use crate::push_toast;
use crate::session::manager::SessionManager;
use crate::toast::{Toast, ToastLevel};
use std::pin::Pin;

pub fn handle_exit<'a>(
    _parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    Box::pin(async { CommandResult::Success("Exiting...".to_string()) })
}

pub fn handle_status<'a>(
    parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    Box::pin(async move {
        if !parsed.args.is_empty() {
            return CommandResult::Error(
                "This command only opens the status dialog. Usage: /status".to_string(),
            );
        }

        CommandResult::Success(String::new())
    })
}

pub fn handle_title<'a>(
    parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    let args = parsed.args.clone();

    Box::pin(async move {
        if !args.is_empty() {
            return CommandResult::Error(
                "This command only opens the terminal title dialog. Usage: /title".to_string(),
            );
        }

        CommandResult::Success(String::new())
    })
}

pub fn handle_sessions<'a>(
    _parsed: &'a ParsedCommand,
    sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    Box::pin(async move {
        let mut sessions = sm.list_sessions();
        sessions.retain(|session| session.parent_id.is_none() && session.archived_at.is_none());
        sessions.sort_by(|a, b| {
            a.workspace_sort_order
                .cmp(&b.workspace_sort_order)
                .then_with(|| a.workspace_id.cmp(&b.workspace_id))
                .then_with(|| b.pinned_at.is_some().cmp(&a.pinned_at.is_some()))
                .then_with(|| b.status.is_active().cmp(&a.status.is_active()))
                .then_with(|| b.updated_at.cmp(&a.updated_at))
        });

        let items: Vec<crate::command::registry::DialogItem> = sessions
            .into_iter()
            .map(|session| {
                let name = if session.pinned_at.is_some() {
                    format!("★ {}", session.title)
                } else {
                    session.title.clone()
                };

                crate::command::registry::DialogItem {
                    id: session.id.clone(),
                    name,
                    group: if session.workspace_name.trim().is_empty() {
                        session.workspace_path.clone()
                    } else {
                        session.workspace_name.clone()
                    },
                    description: String::new(),
                    tip: None,
                    provider_id: session.title.clone(),
                    active: false,
                }
            })
            .collect();

        CommandResult::ShowDialog {
            title: "Sessions".to_string(),
            items,
        }
    })
}

pub fn handle_new<'a>(
    _parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    Box::pin(async move { CommandResult::Success("".to_string()) })
}

pub fn handle_connect<'a>(
    parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    let args = parsed.args.clone();

    Box::pin(async move {
        if !args.is_empty() {
            return CommandResult::Error(
                "This command only opens the connect dialog. Usage: /connect".to_string(),
            );
        }

        let auth_dao = match crate::persistence::AuthDAO::new() {
            Ok(dao) => dao,
            Err(e) => return CommandResult::Error(format!("Failed to load auth config: {}", e)),
        };

        let connected_providers = match auth_dao.load() {
            Ok(providers) => providers,
            Err(e) => return CommandResult::Error(format!("Failed to load providers: {}", e)),
        };
        fn fallback_providers(
        ) -> std::collections::HashMap<String, crate::model::discovery::Provider> {
            use crate::model::discovery::Provider;
            use std::collections::HashMap;

            let mut out: HashMap<String, Provider> = HashMap::new();
            for (id, name) in [
                ("opencode", "OpenCode"),
                ("anthropic", "Anthropic"),
                ("openai", "OpenAI"),
                ("xai", "xAI"),
                ("google", "Google"),
            ] {
                out.insert(
                    id.to_string(),
                    Provider {
                        id: id.to_string(),
                        name: name.to_string(),
                        api: String::new(),
                        doc: String::new(),
                        env: Vec::new(),
                        npm: String::new(),
                        models: HashMap::new(),
                    },
                );
            }
            for integration in crate::model::extensions::ModelExtensions::runtime() {
                out.insert(
                    integration.provider_id().to_string(),
                    Provider {
                        id: integration.provider_id().to_string(),
                        name: integration.provider_name().to_string(),
                        api: String::new(),
                        doc: String::new(),
                        env: Vec::new(),
                        npm: String::new(),
                        models: HashMap::new(),
                    },
                );
            }
            out
        }

        let mut providers_map = match crate::model::discovery::Discovery::new() {
            Ok(discovery) => match discovery.fetch_providers().await {
                Ok(p) => p,
                Err(_) => fallback_providers(),
            },
            Err(_) => fallback_providers(),
        };
        crate::model::extensions::ModelExtensions::augment_runtime_catalog(&mut providers_map);

        const POPULAR_PROVIDERS: &[&str] = &[
            "opencode",
            "anthropic",
            "openai",
            "xai",
            "google",
            "zai-coding-plan",
        ];

        let mut items: Vec<crate::command::registry::DialogItem> = providers_map
            .into_iter()
            .map(|(id, provider)| {
                let group = if crate::model::extensions::ModelExtensions::is_runtime_provider(&id) {
                    "Local"
                } else if POPULAR_PROVIDERS.contains(&id.as_str()) {
                    "Popular"
                } else {
                    "Other"
                };
                let is_connected = connected_providers.contains_key(&id);
                crate::command::registry::DialogItem {
                    id: id.clone(),
                    name: provider.name.clone(),
                    group: group.to_string(),
                    description:
                        crate::model::extensions::ModelExtensions::runtime_provider_description(&id)
                            .unwrap_or(id.as_str())
                            .to_string(),
                    tip: if is_connected {
                        Some("🟢 Connected".to_string())
                    } else {
                        None
                    },
                    provider_id: id.clone(),
                    active: false,
                }
            })
            .collect();

        items.sort_by(|a, b| a.name.cmp(&b.name));

        CommandResult::ShowDialog {
            title: "Connect a provider".to_string(),
            items,
        }
    })
}

pub fn handle_remote<'a>(
    parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    let args = parsed.args.clone();

    Box::pin(async move {
        if !args.is_empty() {
            return CommandResult::Error(
                "This command only opens the remote dialog. Usage: /remote".to_string(),
            );
        }

        CommandResult::Success(String::new())
    })
}

pub fn handle_agents<'a>(
    parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    let args = parsed.args.clone();

    Box::pin(async move {
        if !args.is_empty() {
            return CommandResult::Error(
                "This command only opens the agents dialog. Usage: /agents".to_string(),
            );
        }

        CommandResult::Success(String::new())
    })
}

pub fn handle_models<'a>(
    parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    let parsed = parsed.clone();
    Box::pin(async move { load_models(parsed).await })
}

pub fn handle_variants<'a>(
    parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    let args = parsed.args.clone();
    Box::pin(async move {
        if !args.is_empty() {
            return CommandResult::Error(
                "This command only opens the variants dialog. Usage: /variants".to_string(),
            );
        }
        CommandResult::Success(String::new())
    })
}

pub async fn load_models(parsed: ParsedCommand) -> CommandResult {
    use crate::command::registry::DialogItem;
    use crate::model::discovery::Discovery;
    use crate::model::types::Model as ModelType;
    use crate::persistence::AuthDAO;

    let provider_filter = if parsed.args.is_empty() {
        None
    } else {
        Some(parsed.args[0].clone())
    };

    let active_model_id = parsed.active_model_id.clone();
    let prefs_data = parsed.prefs_data.clone();

    async move {
        let auth_dao = match AuthDAO::new() {
            Ok(dao) => dao,
            Err(e) => return CommandResult::Error(format!("Failed to load auth config: {}", e)),
        };

        let connected_providers = match auth_dao.load() {
            Ok(providers) => providers,
            Err(e) => return CommandResult::Error(format!("Failed to load providers: {}", e)),
        };
        let connected_provider_ids = connected_providers
            .keys()
            .cloned()
            .collect::<std::collections::HashSet<String>>();

        let provider_filter_matches_runtime = provider_filter.as_deref().is_some_and(|filter| {
            let filter = filter.to_ascii_lowercase();
            crate::model::extensions::ModelExtensions::runtime()
                .iter()
                .any(|integration| {
                    integration.provider_id().contains(&filter)
                        || integration
                            .provider_name()
                            .to_ascii_lowercase()
                            .contains(&filter)
                })
        });

        let discovery = Discovery::new();
        let configured_provider_ids = discovery
            .as_ref()
            .map(Discovery::custom_provider_ids)
            .unwrap_or_default();
        let provider_filter_matches_configured = provider_filter.as_deref().is_some_and(|filter| {
            discovery
                .as_ref()
                .is_ok_and(|discovery| discovery.custom_provider_matches_filter(filter))
        });
        let provider_filter_matches_unauthenticated_free = provider_filter.as_deref().is_some_and(
            crate::model::extensions::ModelExtensions::unauthenticated_free_provider_matches_filter,
        );

        let has_runtime = crate::model::extensions::ModelExtensions::runtime()
            .iter()
            .any(|integration| connected_providers.contains_key(integration.provider_id()))
            || (connected_providers.is_empty() && provider_filter.is_none())
            || provider_filter_matches_runtime;
        let has_persistent = connected_providers.keys().any(|provider_id| {
            !crate::model::extensions::ModelExtensions::is_runtime_provider(provider_id)
        }) || provider_filter.is_none()
            || provider_filter_matches_configured
            || provider_filter_matches_unauthenticated_free;

        let snapshot_models = crate::model::effective_catalog::models_for_dialog()
            .ok()
            .flatten();
        let mut models: Vec<ModelType> = if let Some(models) = snapshot_models.as_ref() {
            models.clone()
        } else if has_persistent {
            match discovery.as_ref() {
                Ok(d) => match d.fetch_models().await {
                    Ok(models) => models
                        .into_iter()
                        .filter(|model| {
                            !crate::model::extensions::ModelExtensions::is_runtime_provider(
                                &model.provider_id,
                            )
                        })
                        .collect(),
                    Err(e) => {
                        if has_runtime {
                            push_toast(Toast::new(
                                format!("Skipped models.dev models: {}", e),
                                ToastLevel::Warning,
                                Some(std::time::Duration::from_secs(3)),
                            ));
                            Vec::new()
                        } else {
                            return CommandResult::Error(format!("Failed to fetch models: {}", e));
                        }
                    }
                },
                Err(e) => {
                    if has_runtime {
                        push_toast(Toast::new(
                            format!("Skipped models.dev models: {}", e),
                            ToastLevel::Warning,
                            Some(std::time::Duration::from_secs(3)),
                        ));
                        Vec::new()
                    } else {
                        return CommandResult::Error(format!(
                            "Failed to initialize model discovery: {}",
                            e
                        ));
                    }
                }
            }
        } else {
            Vec::new()
        };

        if let Ok(discovery) = discovery.as_ref() {
            discovery.apply_custom_models_to_dialog(&mut models);
        }

        let mut runtime_errors = Vec::new();
        if has_runtime {
            let runtime_result =
                crate::model::extensions::ModelExtensions::runtime_models_for_dialog_cached().await;
            crate::model::discovery::merge_dialog_models(&mut models, runtime_result.models);
            runtime_errors = runtime_result.errors;
        }

        if snapshot_models.is_none() && !models.is_empty() {
            if let Ok(discovery) = Discovery::new_with_custom(None) {
                if let Ok(snapshot_models) = discovery.fetch_models().await {
                    if let Err(err) =
                        crate::model::effective_catalog::publish_refreshed_models(snapshot_models)
                    {
                        push_toast(Toast::new(
                            format!("Failed to seed model catalog cache: {}", err),
                            ToastLevel::Warning,
                            Some(std::time::Duration::from_secs(3)),
                        ));
                    }
                }
            }
        }

        let prefs = prefs_data;

        let mut model_lookup: std::collections::HashMap<(String, String), ModelType> =
            std::collections::HashMap::new();

        let is_model_selectable = |model: &ModelType| {
            crate::model::discovery::is_model_selectable(
                model,
                &connected_provider_ids,
                &configured_provider_ids,
            ) && crate::model::extensions::ModelExtensions::model_matches_provider_filter(
                model,
                provider_filter.as_deref(),
            )
        };

        for model in &models {
            if is_model_selectable(model) {
                model_lookup.insert((model.provider_id.clone(), model.id.clone()), model.clone());
            }
        }

        let favorites_set = prefs
            .as_ref()
            .map(|p| {
                p.favorite
                    .iter()
                    .map(|m| (m.provider_id.clone(), m.model_id.clone()))
                    .collect::<std::collections::HashSet<_>>()
            })
            .unwrap_or_default();

        let recent_set = prefs
            .as_ref()
            .map(|p| {
                p.recent
                    .iter()
                    .map(|m| (m.provider_id.clone(), m.model_id.clone()))
                    .collect::<std::collections::HashSet<_>>()
            })
            .unwrap_or_default();

        let mut items: Vec<DialogItem> = Vec::new();

        let add_model_item = |items: &mut Vec<DialogItem>, model: &ModelType, group: &str| {
            let is_active = active_model_id.as_ref() == Some(&model.id);
            let is_favorite =
                favorites_set.contains(&(model.provider_id.clone(), model.id.clone()));

            let tip = if is_favorite {
                Some("❤︎".to_string())
            } else {
                None
            };

            let description = model.dialog_description();

            items.push(DialogItem {
                id: model.id.clone(),
                name: model.name.clone(),
                group: group.to_string(),
                description,
                tip,
                provider_id: model.provider_id.clone(),
                active: is_active,
            });
        };

        let favorites_list = prefs
            .as_ref()
            .map(|p| p.favorite.clone())
            .unwrap_or_default();

        let mut favorite_models = Vec::new();
        for fav in &favorites_list {
            if let Some(model) = model_lookup.get(&(fav.provider_id.clone(), fav.model_id.clone()))
            {
                favorite_models.push(model.clone());
            }
        }

        for model in &favorite_models {
            add_model_item(&mut items, model, "Favorite");
        }

        let recent_list = prefs.as_ref().map(|p| p.recent.clone()).unwrap_or_default();

        let mut recent_models = Vec::new();
        for recent in &recent_list {
            if favorites_set.contains(&(recent.provider_id.clone(), recent.model_id.clone())) {
                continue;
            }
            if let Some(model) =
                model_lookup.get(&(recent.provider_id.clone(), recent.model_id.clone()))
            {
                recent_models.push(model.clone());
            }
        }

        for model in &recent_models {
            add_model_item(&mut items, model, "Recent");
        }

        let mut provider_models: std::collections::HashMap<String, Vec<ModelType>> =
            std::collections::HashMap::new();

        for model in models {
            let model_key = (model.provider_id.clone(), model.id.clone());
            if favorites_set.contains(&model_key) || recent_set.contains(&model_key) {
                continue;
            }

            if is_model_selectable(&model) {
                provider_models
                    .entry(model.provider_name.clone())
                    .or_default()
                    .push(model);
            }
        }

        for (provider_name, models_list) in provider_models {
            for model in &models_list {
                add_model_item(&mut items, model, &provider_name);
            }
        }

        items.sort_by(|a, b| {
            let is_a_special = a.group == "Favorite" || a.group == "Recent";
            let is_b_special = b.group == "Favorite" || b.group == "Recent";

            if is_a_special && !is_b_special {
                return std::cmp::Ordering::Less;
            }
            if !is_a_special && is_b_special {
                return std::cmp::Ordering::Greater;
            }

            if is_a_special && is_b_special {
                if a.group == "Favorite" && b.group != "Favorite" {
                    return std::cmp::Ordering::Less;
                }
                if a.group != "Favorite" && b.group == "Favorite" {
                    return std::cmp::Ordering::Greater;
                }
                return std::cmp::Ordering::Equal;
            }

            a.group.cmp(&b.group).then(a.name.cmp(&b.name))
        });

        if items.is_empty() {
            let filter_matches_runtime =
                provider_filter_matches_runtime || provider_filter.is_none();

            if has_runtime && filter_matches_runtime {
                if let Some(err) = runtime_errors.first() {
                    return CommandResult::Error(format!(
                        "Failed to fetch {} models: {}",
                        err.provider_name, err.error
                    ));
                }
            }

            if let Some(filter) = provider_filter {
                CommandResult::Error(format!("No models found for provider: {}", filter))
            } else {
                CommandResult::Error("No models available".to_string())
            }
        } else {
            CommandResult::ShowDialog {
                title: "Available Models".to_string(),
                items,
            }
        }
    }
    .await
}

pub fn handle_themes<'a>(
    parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    let args = parsed.args.clone();

    Box::pin(async move {
        if !args.is_empty() {
            return CommandResult::Error(
                "This command only opens the themes dialog. Usage: /themes".to_string(),
            );
        }

        // The app intercepts /themes to show the dialog.
        CommandResult::Success(String::new())
    })
}

pub fn handle_timeline<'a>(
    parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    let args = parsed.args.clone();

    Box::pin(async move {
        if !args.is_empty() {
            return CommandResult::Error("Usage: /timeline".to_string());
        }

        CommandResult::Success(String::new())
    })
}

pub fn handle_compact<'a>(
    parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    let args = parsed.args.clone();

    Box::pin(async move {
        if !args.is_empty() {
            return CommandResult::Error("Usage: /compact".to_string());
        }

        // The app intercepts /compact because it needs access to the active chat state.
        CommandResult::Success(String::new())
    })
}

pub fn handle_compact_mode<'a>(
    parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    let args = parsed.args.clone();

    Box::pin(async move {
        if !args.is_empty() {
            return CommandResult::Error("Usage: /compact-mode".to_string());
        }

        // The app intercepts /compact-mode to toggle the chat_state.compact_mode flag.
        CommandResult::Success(String::new())
    })
}

pub fn handle_btw<'a>(
    parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    let question = parsed.raw_args().to_string();

    Box::pin(async move {
        if question.trim().is_empty() {
            return CommandResult::Error("Usage: /btw <question>".to_string());
        }

        // The app intercepts /btw because it needs the active provider/model
        // and must run outside the main streaming turn.
        CommandResult::Success(String::new())
    })
}

pub fn handle_fork<'a>(
    parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    let args = parsed.args.clone();

    Box::pin(async move {
        if !args.is_empty() {
            return CommandResult::Error("Usage: /fork".to_string());
        }

        // The app intercepts /fork because it needs access to chat view state.
        CommandResult::Success(String::new())
    })
}

pub fn handle_move<'a>(
    parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    let args = parsed.args.clone();

    Box::pin(async move {
        if !args.is_empty() {
            return CommandResult::Error("Usage: /move".to_string());
        }

        // The app intercepts /move because it needs the active session and TUI state.
        CommandResult::Success(String::new())
    })
}

pub fn handle_skills<'a>(
    parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    let args = parsed.args.clone();

    Box::pin(async move {
        if !args.is_empty() {
            return CommandResult::Error(
                "This command only opens the skills dialog. Usage: /skills".to_string(),
            );
        }

        // The app intercepts /skills to show the dialog.
        CommandResult::Success(String::new())
    })
}

pub fn handle_mcp<'a>(
    parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    let args = parsed.args.clone();

    Box::pin(async move {
        if !args.is_empty() {
            return CommandResult::Error("Usage: /mcp or /mcp auth <name>".to_string());
        }

        CommandResult::Success(String::new())
    })
}

pub fn handle_skill_command<'a>(
    parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    let skill_name = parsed.name.clone();

    Box::pin(async move {
        if let Some(store) = crate::skill::get_skill_store() {
            if let Some(skill) = store.get(&skill_name) {
                return CommandResult::Success(skill.content.clone());
            }
        }

        CommandResult::Error(format!("Unknown command: {}", skill_name))
    })
}

pub fn register_skill_commands(registry: &mut Registry) {
    if let Some(store) = crate::skill::get_skill_store() {
        for skill in store.all() {
            if registry.has_public_command(&skill.name) {
                continue;
            }
            registry.register(Command {
                name: skill.name.clone(),
                description: skill.description.clone().unwrap_or_default(),
                handler: handle_skill_command,
                hidden_tokens: vec![],
                chat_only: false,
            });
            registry.hide_from_autocomplete(skill.name.clone());
        }
    }
}

pub fn handle_rename<'a>(
    parsed: &'a ParsedCommand,
    sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    let session_id = sm.get_current_session_id().cloned();
    let new_title = if parsed.args.is_empty() {
        None
    } else {
        Some(parsed.args.join(" "))
    };

    Box::pin(async move {
        let (Some(sid), Some(title)) = (session_id, new_title) else {
            return CommandResult::Error("Usage: /rename <new title>".to_string());
        };
        match sm.rename_session(&sid, title) {
            Ok(_) => CommandResult::Success(String::new()),
            Err(e) => CommandResult::Error(format!("Failed to rename: {:?}", e)),
        }
    })
}

pub fn handle_copy<'a>(
    _parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    Box::pin(async move { CommandResult::Success("copy".to_string()) })
}

pub fn handle_refreshmodels<'a>(
    _parsed: &'a ParsedCommand,
    _sm: &'a mut SessionManager,
) -> Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
    Box::pin(refresh_models())
}

pub async fn refresh_models() -> CommandResult {
    async move {
        let discovery = match crate::model::discovery::Discovery::new() {
            Ok(d) => d,
            Err(e) => {
                push_toast(Toast::new(
                    format!("Failed to initialize model discovery: {}", e),
                    ToastLevel::Error,
                    Some(std::time::Duration::from_secs(3)),
                ));
                return CommandResult::Success(String::new());
            }
        };

        let (providers_result, runtime_result) = tokio::join!(
            discovery.refresh_cache(),
            crate::model::extensions::ModelExtensions::refresh_runtime_models()
        );

        let mut providers = match providers_result {
            Ok(p) => p,
            Err(e) => {
                push_toast(Toast::new(
                    format!("Skipped models.dev refresh: {}", e),
                    ToastLevel::Warning,
                    Some(std::time::Duration::from_secs(3)),
                ));
                std::collections::HashMap::new()
            }
        };

        let mut runtime_model_count = 0;
        for result in runtime_result {
            match result {
                crate::model::extensions::RefreshResult::Refreshed { model_count, .. } => {
                    runtime_model_count += model_count;
                }
                crate::model::extensions::RefreshResult::Skipped {
                    provider_name,
                    error,
                    ..
                } => {
                    push_toast(Toast::new(
                        format!("Skipped {} refresh: {}", provider_name, error),
                        ToastLevel::Warning,
                        Some(std::time::Duration::from_secs(3)),
                    ));
                }
            }
        }

        crate::model::extensions::ModelExtensions::augment_runtime_catalog(&mut providers);

        let provider_count = providers.len();
        let model_count: usize = providers
            .values()
            .filter(|p| !crate::model::extensions::ModelExtensions::is_runtime_provider(&p.id))
            .map(|p| p.models.len())
            .sum::<usize>()
            + runtime_model_count;

        let models = match crate::model::discovery::Discovery::new_with_custom(None) {
            Ok(discovery) => match discovery.fetch_models().await {
                Ok(models) => models,
                Err(err) => {
                    push_toast(Toast::new(
                        format!("Failed to publish refreshed model catalog: {}", err),
                        ToastLevel::Warning,
                        Some(std::time::Duration::from_secs(3)),
                    ));
                    Vec::new()
                }
            },
            Err(err) => {
                push_toast(Toast::new(
                    format!("Failed to initialize model catalog: {}", err),
                    ToastLevel::Warning,
                    Some(std::time::Duration::from_secs(3)),
                ));
                Vec::new()
            }
        };

        if !models.is_empty() {
            if let Err(err) = crate::model::effective_catalog::publish_refreshed_models(models) {
                push_toast(Toast::new(
                    format!("Failed to publish refreshed model catalog: {}", err),
                    ToastLevel::Warning,
                    Some(std::time::Duration::from_secs(3)),
                ));
            }
        }

        push_toast(Toast::new(
            format!(
                "Models cache refreshed: {} providers, {} models",
                provider_count, model_count
            ),
            ToastLevel::Info,
            Some(std::time::Duration::from_secs(3)),
        ));

        CommandResult::Success(String::new())
    }
    .await
}

pub fn register_all_commands(registry: &mut Registry) {
    registry.register(Command {
        name: "exit".to_string(),
        description: "Quit crabcode".to_string(),
        handler: handle_exit,
        hidden_tokens: vec![],
        chat_only: false,
    });

    registry.register(Command {
        name: "sessions".to_string(),
        description: "List all sessions".to_string(),
        handler: handle_sessions,
        hidden_tokens: vec!["resume".to_string()],
        chat_only: false,
    });

    registry.register(Command {
        name: "new".to_string(),
        description: "Create a new session".to_string(),
        handler: handle_new,
        hidden_tokens: vec![],
        chat_only: false,
    });

    registry.register(Command {
        name: "home".to_string(),
        description: "Switch to home screen".to_string(),
        handler: handle_new,
        hidden_tokens: vec![],
        chat_only: false,
    });

    registry.register(Command {
        name: "connect".to_string(),
        description: "Connect to a model provider".to_string(),
        handler: handle_connect,
        hidden_tokens: vec!["provider".to_string()],
        chat_only: false,
    });

    registry.register(Command {
        name: "remote".to_string(),
        description: "Start a remote host".to_string(),
        handler: handle_remote,
        hidden_tokens: vec!["serve".to_string()],
        chat_only: false,
    });

    registry.register(Command {
        name: "models".to_string(),
        description: "List available models".to_string(),
        handler: handle_models,
        hidden_tokens: vec![],
        chat_only: false,
    });

    registry.register(Command {
        name: "variants".to_string(),
        description: "Switch model variant".to_string(),
        handler: handle_variants,
        hidden_tokens: vec!["reasoning effort".to_string()],
        chat_only: false,
    });

    registry.register(Command {
        name: "status".to_string(),
        description: "Show status".to_string(),
        handler: handle_status,
        hidden_tokens: Vec::new(),
        chat_only: false,
    });

    registry.register(Command {
        name: "agents".to_string(),
        description: "Switch agent".to_string(),
        handler: handle_agents,
        hidden_tokens: vec!["agent".to_string(), "mode".to_string()],
        chat_only: false,
    });

    registry.register(Command {
        name: "themes".to_string(),
        description: "Choose a theme".to_string(),
        handler: handle_themes,
        hidden_tokens: vec![],
        chat_only: false,
    });

    registry.register(Command {
        name: "rename".to_string(),
        description: "Rename the current session".to_string(),
        handler: handle_rename,
        hidden_tokens: vec![],
        chat_only: true,
    });

    registry.register(Command {
        name: "copy".to_string(),
        description: "Copy provider/model id or session details to clipboard".to_string(),
        handler: handle_copy,
        hidden_tokens: vec![],
        chat_only: false,
    });

    registry.register(Command {
        name: "refreshmodels".to_string(),
        description: "Refresh the models.dev cache".to_string(),
        handler: handle_refreshmodels,
        hidden_tokens: vec![],
        chat_only: false,
    });

    registry.register(Command {
        name: "timeline".to_string(),
        description: "Open the message timeline dialog".to_string(),
        handler: handle_timeline,
        hidden_tokens: vec![],
        chat_only: true,
    });

    registry.register(Command {
        name: "compact".to_string(),
        description: "Summarize this session to reduce context".to_string(),
        handler: handle_compact,
        hidden_tokens: vec![],
        chat_only: true,
    });

    registry.register(Command {
        name: "compact-mode".to_string(),
        description: "Toggle compact mode (sticky header + latest user message)".to_string(),
        handler: handle_compact_mode,
        hidden_tokens: vec![],
        chat_only: true,
    });

    registry.register(Command {
        name: "btw".to_string(),
        description: "Ask a side question without interrupting the current task".to_string(),
        handler: handle_btw,
        hidden_tokens: vec![],
        chat_only: false,
    });

    registry.register(Command {
        name: "fork".to_string(),
        description: "Fork the current session".to_string(),
        handler: handle_fork,
        hidden_tokens: vec!["branch".to_string()],
        chat_only: true,
    });

    registry.register(Command {
        name: "move".to_string(),
        description: "Move to another project dir".to_string(),
        handler: handle_move,
        hidden_tokens: vec![],
        chat_only: true,
    });

    registry.register(Command {
        name: "skills".to_string(),
        description: "List available skills".to_string(),
        handler: handle_skills,
        hidden_tokens: vec![],
        chat_only: false,
    });

    registry.register(Command {
        name: "mcp".to_string(),
        description: "List MCP servers (/mcp auth <name> to authenticate)".to_string(),
        handler: handle_mcp,
        hidden_tokens: vec![],
        chat_only: false,
    });

    registry.register(Command {
        name: "title".to_string(),
        description: "Configure terminal title".to_string(),
        handler: handle_title,
        hidden_tokens: vec!["terminal".to_string(), "window".to_string()],
        chat_only: false,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::registry::Registry;

    fn create_registry() -> Registry {
        let mut registry = Registry::new();
        register_all_commands(&mut registry);
        registry
    }

    #[tokio::test]
    async fn test_handle_btw_requires_question() {
        let parsed = ParsedCommand {
            name: "btw".to_string(),
            args: vec![],
            raw: "/btw".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let mut session_manager = SessionManager::new();
        let result = handle_btw(&parsed, &mut session_manager).await;
        assert_eq!(
            result,
            CommandResult::Error("Usage: /btw <question>".to_string())
        );
    }

    #[tokio::test]
    async fn test_handle_btw_with_question_defers_to_app() {
        let parsed = ParsedCommand {
            name: "btw".to_string(),
            args: vec!["also".to_string(), "check".to_string()],
            raw: "/btw also check".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let mut session_manager = SessionManager::new();
        let result = handle_btw(&parsed, &mut session_manager).await;
        // The app intercepts /btw (needs provider/model + side-channel turn).
        assert_eq!(result, CommandResult::Success(String::new()));
    }

    #[test]
    fn test_btw_registered_for_home_and_chat() {
        let registry = create_registry();
        let command = registry.get("btw").expect("/btw registered");
        assert!(!registry.is_chat_only("btw"));
        assert!(!command.description.is_empty());
    }

    #[tokio::test]
    async fn test_handle_exit() {
        let parsed = ParsedCommand {
            name: "exit".to_string(),
            args: vec![],
            raw: "/exit".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let mut session_manager = SessionManager::new();
        let result = handle_exit(&parsed, &mut session_manager).await;
        assert_eq!(result, CommandResult::Success("Exiting...".to_string()));
    }

    #[tokio::test]
    async fn test_handle_sessions() {
        let parsed = ParsedCommand {
            name: "sessions".to_string(),
            args: vec![],
            raw: "/sessions".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let mut session_manager = SessionManager::new();
        let result = handle_sessions(&parsed, &mut session_manager).await;
        match result {
            CommandResult::ShowDialog { title, items } => {
                assert_eq!(title, "Sessions");
                assert!(items.is_empty());
            }
            _ => panic!("Expected ShowDialog"),
        }
    }

    #[tokio::test]
    async fn test_handle_sessions_with_data() {
        let mut session_manager = SessionManager::new();
        session_manager.create_session(Some("session-1".to_string()));
        session_manager.create_session(Some("session-2".to_string()));

        let parsed = ParsedCommand {
            name: "sessions".to_string(),
            args: vec![],
            raw: "/sessions".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let result = handle_sessions(&parsed, &mut session_manager).await;
        match result {
            CommandResult::ShowDialog { title, items } => {
                assert_eq!(title, "Sessions");
                assert_eq!(items.len(), 2);
                assert!(
                    items.iter().any(|item| item.name == "session-1"),
                    "Items: {:?}",
                    items.iter().map(|i| &i.name).collect::<Vec<_>>()
                );
                assert!(items.iter().any(|item| item.name == "session-2"));
            }
            _ => panic!("Expected ShowDialog"),
        }
    }

    #[tokio::test]
    async fn test_handle_sessions_includes_other_workspaces() {
        let mut session_manager = SessionManager::new();
        let current_id = session_manager.create_session(Some("current".to_string()));
        let other_id = session_manager.create_session(Some("other".to_string()));
        let other_session = session_manager.get_session(&other_id).unwrap();
        other_session.workspace_id = 42;
        other_session.workspace_path = "/tmp/other-workspace".to_string();
        other_session.workspace_name = "other-workspace".to_string();

        let parsed = ParsedCommand {
            name: "sessions".to_string(),
            args: vec![],
            raw: "/sessions".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let result = handle_sessions(&parsed, &mut session_manager).await;
        match result {
            CommandResult::ShowDialog { title, items } => {
                assert_eq!(title, "Sessions");
                assert_eq!(items.len(), 2);
                assert!(items.iter().any(|item| item.id == current_id));
                assert!(items
                    .iter()
                    .any(|item| item.id == other_id && item.group == "other-workspace"));
            }
            _ => panic!("Expected ShowDialog"),
        }
    }

    #[tokio::test]
    async fn test_handle_new_no_args() {
        let parsed = ParsedCommand {
            name: "new".to_string(),
            args: vec![],
            raw: "/new".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let mut session_manager = SessionManager::new();
        let result = handle_new(&parsed, &mut session_manager).await;
        match result {
            CommandResult::Success(msg) => {
                assert!(msg.is_empty());
            }
            _ => panic!("Expected Success"),
        }
    }

    #[tokio::test]
    async fn test_handle_new_with_name() {
        let parsed = ParsedCommand {
            name: "new".to_string(),
            args: vec!["my-session".to_string()],
            raw: "/new my-session".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let mut session_manager = SessionManager::new();
        let result = handle_new(&parsed, &mut session_manager).await;
        match result {
            CommandResult::Success(msg) => {
                assert!(msg.is_empty());
            }
            _ => panic!("Expected Success"),
        }
    }

    #[tokio::test]
    async fn test_handle_home() {
        let parsed = ParsedCommand {
            name: "home".to_string(),
            args: vec![],
            raw: "/home".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let mut session_manager = SessionManager::new();
        let result = handle_new(&parsed, &mut session_manager).await;
        match result {
            CommandResult::Success(msg) => {
                assert!(msg.is_empty());
            }
            _ => panic!("Expected Success"),
        }
    }

    #[tokio::test]
    async fn test_handle_connect_no_args() {
        let _ = crate::persistence::AuthDAO::cleanup_test();
        let _ = crate::model::discovery::Discovery::cleanup_test();

        let parsed = ParsedCommand {
            name: "connect".to_string(),
            args: vec![],
            raw: "/connect".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let mut session_manager = SessionManager::new();
        let result = handle_connect(&parsed, &mut session_manager).await;
        match result {
            CommandResult::ShowDialog { title, items } => {
                assert_eq!(title, "Connect a provider");
                assert!(items.iter().any(|item| {
                    item.id == crate::model::extensions::ollama::PROVIDER_ID
                        && item.name == crate::model::extensions::ollama::PROVIDER_NAME
                        && item.group == "Local"
                }));
                if items.len() >= 4 {
                    assert!(items.iter().any(|item| item.id == "anthropic"
                        || item.id == "openai"
                        || item.id == "google"
                        || item.id == "opencode"));
                }
            }
            _ => panic!("Expected ShowDialog"),
        }

        let _ = crate::persistence::AuthDAO::cleanup_test();
        let _ = crate::model::discovery::Discovery::cleanup_test();
    }

    #[tokio::test]
    async fn test_handle_connect_with_args_errors() {
        let _ = crate::persistence::AuthDAO::cleanup_test();

        let parsed = ParsedCommand {
            name: "connect".to_string(),
            args: vec!["nano-gpt".to_string()],
            raw: "/connect nano-gpt".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let mut session_manager = SessionManager::new();
        let result = handle_connect(&parsed, &mut session_manager).await;
        match result {
            CommandResult::Error(msg) => assert!(msg.contains("Usage: /connect")),
            _ => panic!("Expected Error"),
        }

        let _ = crate::persistence::AuthDAO::cleanup_test();
    }

    #[tokio::test]
    async fn test_handle_models() {
        let _ = crate::model::discovery::Discovery::cleanup_test();
        let parsed = ParsedCommand {
            name: "models".to_string(),
            args: vec![],
            raw: "/models".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let mut session_manager = SessionManager::new();
        let result = handle_models(&parsed, &mut session_manager).await;
        match result {
            CommandResult::ShowDialog { title, items } => {
                assert_eq!(title, "Available Models");
                assert!(!items.is_empty());
            }
            CommandResult::Error(_) => {}
            _ => panic!("Expected ShowDialog or Error"),
        }
        let _ = crate::model::discovery::Discovery::cleanup_test();
    }

    #[tokio::test]
    async fn test_handle_models_shows_ollama_without_connection() {
        let _guard = crate::model::extensions::ollama::test_cache_lock();
        let _ = crate::persistence::AuthDAO::cleanup_test();
        crate::model::extensions::ollama::set_cached_models_for_test(vec![
            crate::model::extensions::ollama::OllamaModel {
                id: "llama3.2:latest".to_string(),
                name: "llama3.2:latest".to_string(),
            },
        ]);

        let parsed = ParsedCommand {
            name: "models".to_string(),
            args: vec![],
            raw: "/models".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let mut session_manager = SessionManager::new();
        let result = handle_models(&parsed, &mut session_manager).await;

        match result {
            CommandResult::ShowDialog { title, items } => {
                assert_eq!(title, "Available Models");
                assert!(items.iter().any(|item| {
                    item.id == "llama3.2:latest"
                        && item.provider_id == crate::model::extensions::ollama::PROVIDER_ID
                }));
            }
            other => panic!("Expected Ollama models dialog, got {:?}", other),
        }

        crate::model::extensions::ollama::clear_cache_for_test();
        let _ = crate::persistence::AuthDAO::cleanup_test();
    }

    #[tokio::test]
    async fn test_handle_models_with_filter() {
        let _ = crate::model::discovery::Discovery::cleanup_test();
        let parsed = ParsedCommand {
            name: "models".to_string(),
            args: vec!["open".to_string()],
            raw: "/models open".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let mut session_manager = SessionManager::new();
        let result = handle_models(&parsed, &mut session_manager).await;
        match result {
            CommandResult::ShowDialog { title, items } => {
                assert_eq!(title, "Available Models");
                assert!(!items.is_empty());
            }
            CommandResult::Error(_) => {}
            _ => panic!("Expected ShowDialog or Error"),
        }
        let _ = crate::model::discovery::Discovery::cleanup_test();
    }

    #[tokio::test]
    async fn test_handle_models_cleanup() {
        let _ = crate::persistence::AuthDAO::cleanup_test();
        let _ = crate::model::discovery::Discovery::cleanup_test();
        let parsed = ParsedCommand {
            name: "models".to_string(),
            args: vec![],
            raw: "/models".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let mut session_manager = SessionManager::new();
        let result = handle_models(&parsed, &mut session_manager).await;
        match result {
            CommandResult::ShowDialog { title, items } => {
                assert_eq!(title, "Available Models");
                assert!(!items.is_empty());
            }
            CommandResult::Error(_) => {}
            _ => panic!("Expected ShowDialog or Error"),
        }
        let _ = crate::persistence::AuthDAO::cleanup_test();
        let _ = crate::model::discovery::Discovery::cleanup_test();
    }

    #[tokio::test]
    async fn test_handle_refreshmodels() {
        let _ = crate::model::discovery::Discovery::cleanup_test();
        let parsed = ParsedCommand {
            name: "refreshmodels".to_string(),
            args: vec![],
            raw: "/refreshmodels".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let mut session_manager = SessionManager::new();
        let result = handle_refreshmodels(&parsed, &mut session_manager).await;
        assert_eq!(result, CommandResult::Success(String::new()));
        let _ = crate::model::discovery::Discovery::cleanup_test();
    }

    #[tokio::test]
    async fn test_registry_has_all_commands() {
        let registry = create_registry();
        let names = registry.get_command_names();
        assert_eq!(names.len(), 23);
        assert!(names.contains(&"btw".to_string()));
        assert!(names.contains(&"exit".to_string()));
        assert!(names.contains(&"sessions".to_string()));
        assert!(names.contains(&"new".to_string()));
        assert!(names.contains(&"connect".to_string()));
        assert!(names.contains(&"remote".to_string()));
        assert!(names.contains(&"models".to_string()));
        assert!(names.contains(&"themes".to_string()));
        assert!(names.contains(&"home".to_string()));
        assert!(names.contains(&"rename".to_string()));
        assert!(names.contains(&"copy".to_string()));
        assert!(names.contains(&"refreshmodels".to_string()));
        assert!(names.contains(&"timeline".to_string()));
        assert!(names.contains(&"compact".to_string()));
        assert!(names.contains(&"fork".to_string()));
        assert!(names.contains(&"move".to_string()));
        assert!(names.contains(&"skills".to_string()));
        assert!(names.contains(&"mcp".to_string()));
        assert!(names.contains(&"title".to_string()));
        assert!(names.contains(&"variants".to_string()));
        assert!(names.contains(&"status".to_string()));
        assert!(registry.is_chat_only("compact"));
        assert!(registry.is_chat_only("fork"));
        assert!(!registry.is_chat_only("btw"));
        assert!(registry.is_chat_only("move"));
        assert!(registry.is_chat_only("branch"));
        assert_eq!(registry.get("branch").unwrap().name, "fork");
        assert_eq!(registry.get("provider").unwrap().name, "connect");
    }

    #[tokio::test]
    async fn test_execute_exit_command() {
        let registry = create_registry();
        let parsed = ParsedCommand {
            name: "exit".to_string(),
            args: vec![],
            raw: "/exit".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let mut session_manager = SessionManager::new();
        let result = registry.execute(&parsed, &mut session_manager).await;
        assert_eq!(result, CommandResult::Success("Exiting...".to_string()));
    }

    #[tokio::test]
    async fn test_handle_variants() {
        let registry = create_registry();
        let parsed = ParsedCommand {
            name: "variants".to_string(),
            args: vec![],
            raw: "/variants".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let mut session_manager = SessionManager::new();

        assert!(matches!(
            registry.execute(&parsed, &mut session_manager).await,
            CommandResult::Success(message) if message.is_empty()
        ));
    }

    #[tokio::test]
    async fn test_handle_status() {
        let registry = create_registry();
        let parsed = ParsedCommand {
            name: "status".to_string(),
            args: vec![],
            raw: "/status".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let mut session_manager = SessionManager::new();

        assert!(matches!(
            registry.execute(&parsed, &mut session_manager).await,
            CommandResult::Success(message) if message.is_empty()
        ));
    }

    #[tokio::test]
    async fn test_execute_unknown_command() {
        let registry = create_registry();
        let parsed = ParsedCommand {
            name: "unknown".to_string(),
            args: vec![],
            raw: "/unknown".to_string(),
            prefs_data: None,
            active_model_id: None,
        };
        let mut session_manager = SessionManager::new();
        let result = registry.execute(&parsed, &mut session_manager).await;
        match result {
            CommandResult::Error(msg) => {
                assert!(msg.contains("Unknown command"));
            }
            _ => panic!("Expected Error"),
        }
    }
}
