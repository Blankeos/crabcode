use crate::persistence::HistoryDAO;
use crate::session::types::{MessageRole, Session, SessionStatus};
use std::collections::HashMap;
use std::time::SystemTime;

#[derive(Debug)]
pub enum SessionError {
    NotFound(String),
    PersistenceError(String),
}

impl From<anyhow::Error> for SessionError {
    fn from(err: anyhow::Error) -> Self {
        SessionError::PersistenceError(err.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub parent_id: Option<String>,
    pub title: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub message_count: usize,
    pub workspace_id: i64,
    pub workspace_path: String,
    pub workspace_name: String,
    pub workspace_sort_order: i64,
    pub status: SessionStatus,
    pub pinned_at: Option<SystemTime>,
    pub archived_at: Option<SystemTime>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceInfo {
    pub id: i64,
    pub path: String,
    pub name: String,
    pub sort_order: i64,
    pub last_opened_at: i64,
}

pub struct SessionManager {
    pub sessions: HashMap<String, Session>,
    children_by_parent: HashMap<String, Vec<String>>,
    current_session_id: Option<String>,
    session_counter: usize,
    history_dao: Option<HistoryDAO>,
    id_mapping: HashMap<String, i64>,
    db_id_to_id: HashMap<i64, String>,
    workspace_sort_orders: HashMap<i64, i64>,
    current_workspace_id: i64,
    current_workspace_path: String,
    current_workspace_name: String,
}

impl SessionManager {
    pub fn new() -> Self {
        let current_workspace_path = crate::utils::cwd::current_dir_or_dot()
            .to_string_lossy()
            .to_string();
        let current_workspace_name = workspace_display_name(&current_workspace_path);

        Self {
            sessions: HashMap::new(),
            children_by_parent: HashMap::new(),
            current_session_id: None,
            session_counter: 0,
            history_dao: None,
            id_mapping: HashMap::new(),
            db_id_to_id: HashMap::new(),
            workspace_sort_orders: HashMap::new(),
            current_workspace_id: 0,
            current_workspace_path,
            current_workspace_name,
        }
    }

    pub fn with_history(mut self) -> Result<Self, SessionError> {
        let history_dao =
            HistoryDAO::new().map_err(|e| SessionError::PersistenceError(e.to_string()))?;
        self.current_workspace_id = history_dao.current_workspace_id();
        self.current_workspace_path = history_dao.current_workspace_path().to_string();
        self.current_workspace_name = history_dao.current_workspace_name().to_string();
        self.refresh_workspace_sort_orders(&history_dao)?;
        self.load_sessions_from_db(&history_dao)?;
        self.history_dao = Some(history_dao);
        Ok(self)
    }

    fn refresh_workspace_sort_orders(&mut self, dao: &HistoryDAO) -> Result<(), SessionError> {
        let workspaces = dao
            .list_workspaces()
            .map_err(|e| SessionError::PersistenceError(e.to_string()))?;
        self.workspace_sort_orders = workspaces
            .into_iter()
            .map(|workspace| (workspace.id, workspace.sort_order))
            .collect();
        Ok(())
    }

    fn load_sessions_from_db(&mut self, dao: &HistoryDAO) -> Result<(), SessionError> {
        let db_sessions = dao
            .list_sessions()
            .map_err(|e| SessionError::PersistenceError(e.to_string()))?;

        for db_session in db_sessions {
            let messages = dao
                .get_messages(db_session.id)
                .map_err(|e| SessionError::PersistenceError(e.to_string()))?;

            let mut session = if messages.is_empty() {
                Session::with_title(db_session.name.clone())
            } else {
                crate::persistence::persistence_to_session(db_session.clone(), messages)
                    .map_err(|e| SessionError::PersistenceError(e.to_string()))?
            };

            session.id = db_session.session_identifier.clone();
            session.parent_id = db_session.parent_session_identifier.clone();
            session.title = db_session.name;
            session.created_at = std::time::UNIX_EPOCH
                + std::time::Duration::from_secs(db_session.created_at as u64);
            session.updated_at = std::time::UNIX_EPOCH
                + std::time::Duration::from_secs(db_session.updated_at as u64);
            session.workspace_id = db_session.workspace_id;
            session.workspace_path = db_session.workspace_path;
            session.workspace_name = db_session.workspace_name;
            session.workspace_sort_order = db_session.workspace_sort_order;
            session.status = SessionStatus::from_str(&db_session.status);
            if session.status.is_active() {
                session.status = SessionStatus::Interrupted;
                if let Some(message) = session
                    .messages
                    .iter_mut()
                    .rev()
                    .find(|message| message.role == MessageRole::Assistant)
                {
                    message.mark_complete();
                    message.mark_interrupted();
                    message.mark_running_tool_parts_failed(
                        "Session interrupted before the tool returned a result",
                    );
                }
                let _ = dao.set_session_status(db_session.id, session.status.as_str(), None);
                let persistence_messages: Vec<crate::persistence::Message> = session
                    .messages
                    .clone()
                    .into_iter()
                    .map(|message| {
                        let mut db_message: crate::persistence::Message = message.into();
                        db_message.session_id = db_session.id;
                        db_message
                    })
                    .collect();
                let _ = dao.replace_messages(db_session.id, &persistence_messages);
            }
            session.pinned_at = db_session
                .pinned_at
                .map(|ts| std::time::UNIX_EPOCH + std::time::Duration::from_secs(ts as u64));
            session.archived_at = db_session
                .archived_at
                .map(|ts| std::time::UNIX_EPOCH + std::time::Duration::from_secs(ts as u64));

            let session_id = session.id.clone();
            let parent_id = session.parent_id.clone();
            self.sessions.insert(session_id.clone(), session);
            if let Some(parent_id) = parent_id {
                self.index_child_session(&parent_id, &session_id);
            }
            self.id_mapping.insert(session_id.clone(), db_session.id);
            self.db_id_to_id.insert(db_session.id, session_id);

            self.session_counter += 1;
        }

        self.sort_child_session_indexes();

        Ok(())
    }

    fn index_child_session(&mut self, parent_id: &str, child_id: &str) {
        let children = self
            .children_by_parent
            .entry(parent_id.to_string())
            .or_default();
        if !children.iter().any(|id| id == child_id) {
            children.push(child_id.to_string());
        }
    }

    fn unindex_child_session(&mut self, parent_id: &str, child_id: &str) {
        let should_remove = if let Some(children) = self.children_by_parent.get_mut(parent_id) {
            children.retain(|id| id != child_id);
            children.is_empty()
        } else {
            false
        };

        if should_remove {
            self.children_by_parent.remove(parent_id);
        }
    }

    fn sort_child_session_indexes(&mut self) {
        let sessions = &self.sessions;
        for children in self.children_by_parent.values_mut() {
            children.sort_by(|a, b| {
                let a_session = sessions.get(a);
                let b_session = sessions.get(b);
                match (a_session, b_session) {
                    (Some(a_session), Some(b_session)) => a_session
                        .created_at
                        .cmp(&b_session.created_at)
                        .then_with(|| a.cmp(b)),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => a.cmp(b),
                }
            });
        }
    }

    fn insert_child_session_index_sorted(&mut self, parent_id: &str, child_id: &str) {
        self.index_child_session(parent_id, child_id);
        self.sort_child_session_indexes();
    }

    fn session_info_from_session(
        id: &str,
        session: &Session,
        workspace_sort_order: i64,
    ) -> SessionInfo {
        SessionInfo {
            id: id.to_string(),
            parent_id: session.parent_id.clone(),
            title: session.title.clone(),
            created_at: session.created_at,
            updated_at: session.updated_at,
            message_count: session.messages.len(),
            workspace_id: session.workspace_id,
            workspace_path: session.workspace_path.clone(),
            workspace_name: session.workspace_name.clone(),
            workspace_sort_order,
            status: session.status,
            pinned_at: session.pinned_at,
            archived_at: session.archived_at,
        }
    }

    pub fn create_session(&mut self, name: Option<String>) -> String {
        self.create_session_record(name, None, None, true)
    }

    pub fn create_child_session(
        &mut self,
        parent_id: String,
        session_id: String,
        name: String,
    ) -> String {
        self.create_session_record(Some(name), Some(session_id), Some(parent_id), false)
    }

    fn create_session_record(
        &mut self,
        name: Option<String>,
        requested_id: Option<String>,
        parent_id: Option<String>,
        make_current: bool,
    ) -> String {
        self.session_counter += 1;
        let title = name
            .clone()
            .unwrap_or_else(|| format!("session-{}", self.session_counter));

        let session_id = requested_id.unwrap_or_else(cuid2::create_id);

        let mut session = Session::with_title(title.clone());
        session.id = session_id.clone();
        session.parent_id = parent_id.clone();
        session.workspace_id = self.current_workspace_id;
        session.workspace_path = self.current_workspace_path.clone();
        session.workspace_name = self.current_workspace_name.clone();
        session.workspace_sort_order = self.workspace_sort_order(self.current_workspace_id);

        self.sessions.insert(session_id.clone(), session);
        if let Some(ref parent_id) = parent_id {
            self.insert_child_session_index_sorted(parent_id, &session_id);
        }
        if make_current {
            self.current_session_id = Some(session_id.clone());
        }

        if let Some(ref dao) = self.history_dao {
            let db_id = dao
                .create_session_with_parent_in_workspace(
                    &session_id,
                    title.clone(),
                    parent_id.as_deref(),
                    self.current_workspace_id,
                )
                .unwrap_or_else(|_| self.session_counter as i64);
            self.id_mapping.insert(session_id.clone(), db_id);
            self.db_id_to_id.insert(db_id, session_id.clone());
        }

        session_id
    }

    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        self.sessions
            .iter()
            .map(|(id, session)| {
                Self::session_info_from_session(
                    id,
                    session,
                    self.workspace_sort_order(session.workspace_id),
                )
            })
            .collect()
    }

    pub fn get_current_session(&mut self) -> Option<&mut Session> {
        if let Some(id) = &self.current_session_id {
            self.sessions.get_mut(id)
        } else {
            None
        }
    }

    pub fn get_session(&mut self, id: &str) -> Option<&mut Session> {
        self.sessions.get_mut(id)
    }

    pub fn get_session_ref(&self, id: &str) -> Option<&Session> {
        self.sessions.get(id)
    }

    pub fn parent_id_of(&self, id: &str) -> Option<&str> {
        self.sessions.get(id).and_then(|s| s.parent_id.as_deref())
    }

    pub fn root_session_id_for(&self, id: &str) -> Option<String> {
        let mut current_id = id;
        loop {
            let session = self.sessions.get(current_id)?;
            let Some(parent_id) = session.parent_id.as_deref() else {
                return Some(current_id.to_string());
            };
            current_id = parent_id;
        }
    }

    pub fn descendant_sessions(&self, parent_id: &str) -> Vec<SessionInfo> {
        let mut descendants = Vec::new();
        self.collect_descendant_sessions(parent_id, &mut descendants);
        descendants
    }

    fn collect_descendant_sessions(&self, parent_id: &str, descendants: &mut Vec<SessionInfo>) {
        let Some(children) = self.children_by_parent.get(parent_id) else {
            return;
        };

        for child_id in children {
            if let Some(session) = self.sessions.get(child_id) {
                descendants.push(Self::session_info_from_session(
                    child_id,
                    session,
                    self.workspace_sort_order(session.workspace_id),
                ));
                self.collect_descendant_sessions(child_id, descendants);
            }
        }
    }

    pub fn child_sessions(&self, parent_id: &str) -> Vec<SessionInfo> {
        self.children_by_parent
            .get(parent_id)
            .into_iter()
            .flat_map(|children| children.iter())
            .filter_map(|id| {
                self.sessions.get(id).map(|session| {
                    Self::session_info_from_session(
                        id,
                        session,
                        self.workspace_sort_order(session.workspace_id),
                    )
                })
            })
            .collect()
    }

    pub fn switch_session(&mut self, id: &str) -> bool {
        if self.sessions.contains_key(id) {
            self.current_session_id = Some(id.to_string());
            true
        } else {
            false
        }
    }

    pub fn get_current_session_id(&self) -> Option<&String> {
        self.current_session_id.as_ref()
    }

    pub fn current_workspace_id(&self) -> i64 {
        self.current_workspace_id
    }

    pub fn current_workspace_path(&self) -> &str {
        &self.current_workspace_path
    }

    pub fn current_workspace_name(&self) -> &str {
        &self.current_workspace_name
    }

    pub fn list_workspaces(&self) -> Vec<WorkspaceInfo> {
        let mut workspaces = self
            .history_dao
            .as_ref()
            .and_then(|dao| dao.list_workspaces().ok())
            .map(|workspaces| {
                workspaces
                    .into_iter()
                    .filter(|workspace| workspace.archived_at.is_none())
                    .map(|workspace| WorkspaceInfo {
                        id: workspace.id,
                        path: workspace.root_path,
                        name: workspace.display_name,
                        sort_order: workspace.sort_order,
                        last_opened_at: workspace.last_opened_at,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if !workspaces
            .iter()
            .any(|workspace| workspace.path == self.current_workspace_path)
        {
            workspaces.push(WorkspaceInfo {
                id: self.current_workspace_id,
                path: self.current_workspace_path.clone(),
                name: self.current_workspace_name.clone(),
                sort_order: self.workspace_sort_order(self.current_workspace_id),
                last_opened_at: i64::MAX,
            });
        }

        workspaces.sort_by(|a, b| {
            a.sort_order
                .cmp(&b.sort_order)
                .then_with(|| a.id.cmp(&b.id))
                .then_with(|| a.name.cmp(&b.name))
        });
        workspaces
    }

    pub fn switch_current_workspace_path(&mut self, root_path: &str) -> Result<(), SessionError> {
        let root_path = root_path.trim();
        if root_path.is_empty() {
            return Err(SessionError::PersistenceError(
                "workspace path cannot be empty".to_string(),
            ));
        }

        if let Some(ref dao) = self.history_dao {
            let workspace = dao
                .ensure_workspace_path(root_path)
                .map_err(|e| SessionError::PersistenceError(e.to_string()))?;
            let workspaces = dao
                .list_workspaces()
                .map_err(|e| SessionError::PersistenceError(e.to_string()))?;
            self.workspace_sort_orders = workspaces
                .into_iter()
                .map(|workspace| (workspace.id, workspace.sort_order))
                .collect();
            self.current_workspace_id = workspace.id;
            self.current_workspace_path = workspace.root_path;
            self.current_workspace_name = workspace.display_name;
        } else {
            self.current_workspace_id = 0;
            self.current_workspace_path = root_path.to_string();
            self.current_workspace_name = workspace_display_name(root_path);
        }

        Ok(())
    }

    pub fn workspace_sort_order(&self, workspace_id: i64) -> i64 {
        self.workspace_sort_orders
            .get(&workspace_id)
            .copied()
            .unwrap_or(workspace_id)
    }

    pub fn move_workspace_sort_order(
        &mut self,
        workspace_id: i64,
        offset: isize,
    ) -> Result<bool, SessionError> {
        let moved = if let Some(dao) = self.history_dao.as_ref() {
            let moved = dao
                .move_workspace_sort_order(workspace_id, offset)
                .map_err(|e| SessionError::PersistenceError(e.to_string()))?;
            let workspaces = dao
                .list_workspaces()
                .map_err(|e| SessionError::PersistenceError(e.to_string()))?;
            self.workspace_sort_orders = workspaces
                .into_iter()
                .map(|workspace| (workspace.id, workspace.sort_order))
                .collect();
            moved
        } else {
            false
        };

        let workspace_sort_orders = self.workspace_sort_orders.clone();
        for session in self.sessions.values_mut() {
            session.workspace_sort_order = workspace_sort_orders
                .get(&session.workspace_id)
                .copied()
                .unwrap_or(session.workspace_id);
        }

        Ok(moved)
    }

    pub fn move_session_to_workspace(
        &mut self,
        session_id: &str,
        workspace_id: i64,
    ) -> Result<bool, SessionError> {
        if !self.sessions.contains_key(session_id) {
            return Err(SessionError::NotFound(session_id.to_string()));
        }

        let Some(workspace) = self
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.id == workspace_id)
        else {
            return Ok(false);
        };

        if let Some(session) = self.sessions.get(session_id) {
            if session.workspace_id == workspace_id {
                return Ok(false);
            }
        }

        if let Some(ref dao) = self.history_dao {
            if let Some(db_id) = self.id_mapping.get(session_id) {
                let moved = dao
                    .move_session_to_workspace(*db_id, workspace_id)
                    .map_err(|e| SessionError::PersistenceError(e.to_string()))?;
                if !moved {
                    return Ok(false);
                }
            }
        }

        if let Some(session) = self.sessions.get_mut(session_id) {
            session.workspace_id = workspace.id;
            session.workspace_path = workspace.path;
            session.workspace_name = workspace.name;
            session.workspace_sort_order = workspace.sort_order;
            session.updated_at = SystemTime::now();
        }

        Ok(true)
    }

    pub fn clear_current_session(&mut self) {
        self.current_session_id = None;
    }

    pub fn get_db_id(&self, session_id: &str) -> Option<i64> {
        self.id_mapping.get(session_id).copied()
    }

    pub fn add_message_to_current_session(
        &mut self,
        message: &crate::session::types::Message,
    ) -> Result<(), SessionError> {
        let Some(session_id) = self.current_session_id.clone() else {
            return Ok(());
        };
        self.add_message_to_session(&session_id, message)
    }

    pub fn add_message_to_session(
        &mut self,
        session_id: &str,
        message: &crate::session::types::Message,
    ) -> Result<(), SessionError> {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.add_message(message.clone());
        } else {
            return Err(SessionError::NotFound(session_id.to_string()));
        }

        if let Some(ref dao) = self.history_dao {
            if let Some(db_id) = self.id_mapping.get(session_id) {
                let mut db_message: crate::persistence::Message = message.clone().into();
                db_message.session_id = *db_id;
                let _ = dao
                    .add_message(&db_message)
                    .map_err(|e| SessionError::PersistenceError(e.to_string()));
            }
        }
        Ok(())
    }

    pub fn replace_session_messages(
        &mut self,
        session_id: &str,
        messages: Vec<crate::session::types::Message>,
    ) -> Result<(), SessionError> {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.messages = messages.clone();
            session.updated_at = SystemTime::now();
        } else {
            return Err(SessionError::NotFound(session_id.to_string()));
        }

        if let Some(ref dao) = self.history_dao {
            if let Some(db_id) = self.id_mapping.get(session_id) {
                let persistence_messages: Vec<crate::persistence::Message> = messages
                    .into_iter()
                    .map(|message| {
                        let mut db_message: crate::persistence::Message = message.into();
                        db_message.session_id = *db_id;
                        db_message
                    })
                    .collect();

                dao.replace_messages(*db_id, &persistence_messages)
                    .map_err(|e| SessionError::PersistenceError(e.to_string()))?;
            }
        }

        Ok(())
    }

    pub fn set_session_status(
        &mut self,
        id: &str,
        status: SessionStatus,
        last_error: Option<&str>,
    ) -> Result<(), SessionError> {
        if let Some(session) = self.sessions.get_mut(id) {
            session.status = status;
            session.updated_at = SystemTime::now();
        } else {
            return Err(SessionError::NotFound(id.to_string()));
        }

        if let Some(ref dao) = self.history_dao {
            if let Some(db_id) = self.id_mapping.get(id) {
                let _ = dao.set_session_status(*db_id, status.as_str(), last_error);
            }
        }

        Ok(())
    }

    pub fn toggle_session_pin(&mut self, id: &str) -> Result<bool, SessionError> {
        let pinned = if let Some(session) = self.sessions.get_mut(id) {
            if session.pinned_at.is_some() {
                session.pinned_at = None;
                false
            } else {
                session.pinned_at = Some(SystemTime::now());
                true
            }
        } else {
            return Err(SessionError::NotFound(id.to_string()));
        };

        if let Some(ref dao) = self.history_dao {
            if let Some(db_id) = self.id_mapping.get(id) {
                let pinned_at = dao.set_session_pinned(*db_id, pinned).ok().flatten();
                if let Some(session) = self.sessions.get_mut(id) {
                    session.pinned_at = pinned_at.map(|ts| {
                        std::time::UNIX_EPOCH + std::time::Duration::from_secs(ts as u64)
                    });
                }
            }
        }

        Ok(pinned)
    }

    pub fn set_session_archived(&mut self, id: &str, archived: bool) -> Result<(), SessionError> {
        if let Some(session) = self.sessions.get_mut(id) {
            session.archived_at = if archived {
                Some(SystemTime::now())
            } else {
                None
            };
            session.updated_at = SystemTime::now();
        } else {
            return Err(SessionError::NotFound(id.to_string()));
        }

        if let Some(ref dao) = self.history_dao {
            if let Some(db_id) = self.id_mapping.get(id) {
                let archived_at = dao.set_session_archived(*db_id, archived).ok().flatten();
                if let Some(session) = self.sessions.get_mut(id) {
                    session.archived_at = archived_at.map(|ts| {
                        std::time::UNIX_EPOCH + std::time::Duration::from_secs(ts as u64)
                    });
                }
            }
        }

        Ok(())
    }

    pub fn set_workspace_archived(
        &mut self,
        root_path: &str,
        archived: bool,
    ) -> Result<bool, SessionError> {
        let root_path = root_path.trim();
        if root_path.is_empty() {
            return Err(SessionError::PersistenceError(
                "workspace path cannot be empty".to_string(),
            ));
        }

        let archived_at = if archived {
            Some(SystemTime::now())
        } else {
            None
        };
        let mut changed = false;

        if let Some(ref dao) = self.history_dao {
            changed = dao
                .set_workspace_archived(root_path, archived)
                .map_err(|e| SessionError::PersistenceError(e.to_string()))?;
            let workspaces = dao
                .list_workspaces()
                .map_err(|e| SessionError::PersistenceError(e.to_string()))?;
            self.workspace_sort_orders = workspaces
                .iter()
                .map(|workspace| (workspace.id, workspace.sort_order))
                .collect();

            if archived && self.current_workspace_path == root_path {
                if let Some(workspace) = workspaces
                    .iter()
                    .find(|workspace| {
                        workspace.archived_at.is_none() && workspace.root_path != root_path
                    })
                    .cloned()
                {
                    self.current_workspace_id = workspace.id;
                    self.current_workspace_path = workspace.root_path;
                    self.current_workspace_name = workspace.display_name;
                }
            }
        }

        let current_session_id = self.current_session_id.clone();
        let mut current_session_archived = false;
        for session in self.sessions.values_mut() {
            if session.workspace_path == root_path {
                session.archived_at = archived_at;
                if current_session_id.as_deref() == Some(session.id.as_str()) && archived {
                    current_session_archived = true;
                }
            }
        }

        if current_session_archived {
            self.current_session_id = None;
        }

        Ok(changed)
    }

    pub fn rename_session(&mut self, id: &str, new_title: String) -> Result<(), SessionError> {
        if let Some(session) = self.sessions.get_mut(id) {
            session.title = new_title.clone();
            session.updated_at = SystemTime::now();

            if let Some(ref dao) = self.history_dao {
                if let Some(db_id) = self.id_mapping.get(id) {
                    let _ = dao.rename_session(*db_id, new_title);
                }
            }

            Ok(())
        } else {
            Err(SessionError::NotFound(id.to_string()))
        }
    }

    pub fn delete_session(&mut self, id: &str) -> bool {
        if let Some(db_id) = self.id_mapping.get(id) {
            if let Some(ref dao) = self.history_dao {
                let _ = dao.delete_session(*db_id);
            }
        }

        let parent_id = self
            .sessions
            .get(id)
            .and_then(|session| session.parent_id.clone());

        if self.sessions.remove(id).is_some() {
            if let Some(parent_id) = parent_id {
                self.unindex_child_session(&parent_id, id);
            }
            self.children_by_parent.remove(id);
            if let Some(db_id) = self.id_mapping.remove(id) {
                self.db_id_to_id.remove(&db_id);
            }
            if self.current_session_id.as_ref() == Some(&id.to_string()) {
                self.current_session_id = None;
            }
            true
        } else {
            false
        }
    }
}

fn workspace_display_name(root_path: &str) -> String {
    std::path::Path::new(root_path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(root_path)
        .to_string()
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_manager_new() {
        let manager = SessionManager::new();
        assert!(manager.sessions.is_empty());
        assert!(manager.current_session_id.is_none());
        assert_eq!(manager.session_counter, 0);
    }

    #[test]
    fn test_create_session_default_name() {
        let mut manager = SessionManager::new();
        let id = manager.create_session(None);
        assert!(!id.is_empty());
        assert!(manager.sessions.contains_key(&id));
        assert_eq!(manager.current_session_id, Some(id));
    }

    #[test]
    fn test_create_session_custom_name() {
        let mut manager = SessionManager::new();
        let id = manager.create_session(Some("my-session".to_string()));
        assert!(!id.is_empty());
        assert!(manager.sessions.contains_key(&id));
        assert_eq!(manager.current_session_id, Some(id.clone()));
        let session = manager.get_session(&id).unwrap();
        assert_eq!(session.title, "my-session");
    }

    #[test]
    fn test_create_multiple_sessions() {
        let mut manager = SessionManager::new();
        let id1 = manager.create_session(None);
        let id2 = manager.create_session(None);
        let id3 = manager.create_session(None);

        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_ne!(id1, id3);
        assert_eq!(manager.sessions.len(), 3);
    }

    #[test]
    fn test_list_sessions_empty() {
        let manager = SessionManager::new();
        let sessions = manager.list_sessions();
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_list_sessions() {
        let mut manager = SessionManager::new();
        manager.create_session(Some("session-1".to_string()));
        manager.create_session(Some("session-2".to_string()));

        let sessions = manager.list_sessions();
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn test_get_current_session_none() {
        let mut manager = SessionManager::new();
        assert!(manager.get_current_session().is_none());
    }

    #[test]
    fn test_get_current_session_exists() {
        let mut manager = SessionManager::new();
        manager.create_session(Some("test".to_string()));
        assert!(manager.get_current_session().is_some());
    }

    #[test]
    fn test_get_session() {
        let mut manager = SessionManager::new();
        let id = manager.create_session(Some("test".to_string()));
        assert!(manager.get_session(&id).is_some());
        assert!(manager.get_session("nonexistent").is_none());
    }

    #[test]
    fn test_switch_session() {
        let mut manager = SessionManager::new();
        let id1 = manager.create_session(Some("session-1".to_string()));
        let id2 = manager.create_session(Some("session-2".to_string()));

        assert!(manager.switch_session(&id1));
        assert_eq!(manager.current_session_id, Some(id1.clone()));

        assert!(manager.switch_session(&id2));
        assert_eq!(manager.current_session_id, Some(id2.clone()));

        assert!(!manager.switch_session("nonexistent"));
    }

    #[test]
    fn test_delete_session() {
        let mut manager = SessionManager::new();
        let id1 = manager.create_session(Some("session-1".to_string()));
        let id2 = manager.create_session(Some("session-2".to_string()));

        assert!(manager.delete_session(&id1));
        assert!(!manager.sessions.contains_key(&id1));
        assert!(manager.sessions.contains_key(&id2));
    }

    #[test]
    fn test_delete_current_session() {
        let mut manager = SessionManager::new();
        let id1 = manager.create_session(Some("session-1".to_string()));
        let _id2 = manager.create_session(Some("session-2".to_string()));

        manager.switch_session(&id1);
        assert!(manager.delete_session(&id1));
        assert!(manager.current_session_id.is_none());
    }
}
