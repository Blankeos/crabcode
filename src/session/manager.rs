use crate::persistence::HistoryDAO;
use crate::session::types::{Session, SessionStatus};
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

#[derive(Debug)]
pub struct SessionInfo {
    pub id: String,
    pub title: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub message_count: usize,
    pub workspace_id: i64,
    pub workspace_path: String,
    pub workspace_name: String,
    pub status: SessionStatus,
    pub pinned_at: Option<SystemTime>,
    pub archived_at: Option<SystemTime>,
}

pub struct SessionManager {
    pub sessions: HashMap<String, Session>,
    current_session_id: Option<String>,
    session_counter: usize,
    history_dao: Option<HistoryDAO>,
    id_mapping: HashMap<String, i64>,
    db_id_to_id: HashMap<i64, String>,
    current_workspace_id: i64,
    current_workspace_path: String,
    current_workspace_name: String,
}

impl SessionManager {
    pub fn new() -> Self {
        let current_workspace_path = std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .to_string_lossy()
            .to_string();
        let current_workspace_name = workspace_display_name(&current_workspace_path);

        Self {
            sessions: HashMap::new(),
            current_session_id: None,
            session_counter: 0,
            history_dao: None,
            id_mapping: HashMap::new(),
            db_id_to_id: HashMap::new(),
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
        self.load_sessions_from_db(&history_dao)?;
        self.history_dao = Some(history_dao);
        Ok(self)
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
            session.title = db_session.name;
            session.created_at = std::time::UNIX_EPOCH
                + std::time::Duration::from_secs(db_session.created_at as u64);
            session.updated_at = std::time::UNIX_EPOCH
                + std::time::Duration::from_secs(db_session.updated_at as u64);
            session.workspace_id = db_session.workspace_id;
            session.workspace_path = db_session.workspace_path;
            session.workspace_name = db_session.workspace_name;
            session.status = SessionStatus::from_str(&db_session.status);
            if session.status.is_active() {
                session.status = SessionStatus::Interrupted;
                let _ = dao.set_session_status(db_session.id, session.status.as_str(), None);
            }
            session.pinned_at = db_session
                .pinned_at
                .map(|ts| std::time::UNIX_EPOCH + std::time::Duration::from_secs(ts as u64));
            session.archived_at = db_session
                .archived_at
                .map(|ts| std::time::UNIX_EPOCH + std::time::Duration::from_secs(ts as u64));

            let session_id = session.id.clone();
            self.sessions.insert(session_id.clone(), session);
            self.id_mapping.insert(session_id.clone(), db_session.id);
            self.db_id_to_id.insert(db_session.id, session_id);

            self.session_counter += 1;
        }

        Ok(())
    }

    pub fn create_session(&mut self, name: Option<String>) -> String {
        self.session_counter += 1;
        let title = name
            .clone()
            .unwrap_or_else(|| format!("session-{}", self.session_counter));

        let session_id = cuid2::create_id();

        let mut session = Session::with_title(title.clone());
        session.id = session_id.clone();
        session.workspace_id = self.current_workspace_id;
        session.workspace_path = self.current_workspace_path.clone();
        session.workspace_name = self.current_workspace_name.clone();

        self.sessions.insert(session_id.clone(), session);
        self.current_session_id = Some(session_id.clone());

        if let Some(ref dao) = self.history_dao {
            let db_id = dao
                .create_session(&session_id, title.clone())
                .unwrap_or_else(|_| self.session_counter as i64);
            self.id_mapping.insert(session_id.clone(), db_id);
            self.db_id_to_id.insert(db_id, session_id.clone());
        }

        session_id
    }

    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        self.sessions
            .iter()
            .map(|(id, session)| SessionInfo {
                id: id.clone(),
                title: session.title.clone(),
                created_at: session.created_at,
                updated_at: session.updated_at,
                message_count: session.messages.len(),
                workspace_id: session.workspace_id,
                workspace_path: session.workspace_path.clone(),
                workspace_name: session.workspace_name.clone(),
                status: session.status,
                pinned_at: session.pinned_at,
                archived_at: session.archived_at,
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

        if self.sessions.remove(id).is_some() {
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
