use super::models::find_selectable_model;
use super::models::model_config_option;
use super::models::model_context_window;
use super::models::reasoning_config_option;
use super::models::resolved_reasoning;
use super::modes::mode_description;
use super::AcpService;
use super::AcpSession;
use agent_client_protocol::schema::v1::SessionConfigOption;
use agent_client_protocol::schema::v1::SessionConfigOptionCategory;
use agent_client_protocol::schema::v1::SessionConfigSelectOption;
use agent_client_protocol::schema::v1::SetSessionConfigOptionResponse;
use agent_client_protocol::Error;

pub(super) fn session_config_options(session: &AcpSession) -> Vec<SessionConfigOption> {
    let mut mode_options = session
        .config
        .merged_config
        .agent_registry
        .visible_primary_agents()
        .into_iter()
        .map(|agent| {
            SessionConfigSelectOption::new(agent.name.clone(), agent.name.clone())
                .description(mode_description(agent))
        })
        .collect::<Vec<_>>();
    mode_options.sort_by(|left, right| left.name.cmp(&right.name));

    let mut options = vec![
        SessionConfigOption::select("mode", "Mode", session.agent.clone(), mode_options)
            .category(SessionConfigOptionCategory::Mode),
        model_config_option(&session.models, &session.provider, &session.model),
    ];
    options.push(reasoning_config_option(session));
    options
}

impl AcpService {
    pub async fn set_mode(
        &self,
        session_id: &str,
        mode_id: &str,
    ) -> Result<SetSessionConfigOptionResponse, Error> {
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .attachments
            .get_mut(session_id)
            .ok_or_else(|| Error::invalid_params().data("unknown session"))?;
        if session
            .config
            .merged_config
            .agent_registry
            .primary_agent(mode_id)
            .is_none_or(|agent| agent.hidden)
        {
            return Err(Error::invalid_params().data("unknown ACP mode"));
        }
        session.agent = mode_id.to_string();
        Ok(SetSessionConfigOptionResponse::new(session_config_options(
            session,
        )))
    }

    pub async fn set_model(
        &self,
        session_id: &str,
        model_ref: &str,
    ) -> Result<SetSessionConfigOptionResponse, Error> {
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .attachments
            .get_mut(session_id)
            .ok_or_else(|| Error::invalid_params().data("unknown session"))?;
        let model = find_selectable_model(&session.models, model_ref)?;
        session.provider.clone_from(&model.provider_id);
        session.model.clone_from(&model.id);
        session.reasoning = resolved_reasoning(session, session.reasoning_selection);
        session.context_window = model_context_window(
            &session.config,
            &session.models,
            &session.provider,
            &session.model,
        );
        Ok(SetSessionConfigOptionResponse::new(session_config_options(
            session,
        )))
    }

    pub async fn set_reasoning_effort(
        &self,
        session_id: &str,
        value: &str,
    ) -> Result<SetSessionConfigOptionResponse, Error> {
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .attachments
            .get_mut(session_id)
            .ok_or_else(|| Error::invalid_params().data("unknown session"))?;
        let requested = value
            .parse::<crate::model::reasoning::ReasoningEffort>()
            .map_err(|_| Error::invalid_params().data("invalid reasoning effort"))?;
        session.reasoning_selection = requested;
        session.reasoning = resolved_reasoning(session, requested);
        Ok(SetSessionConfigOptionResponse::new(session_config_options(
            session,
        )))
    }
}
