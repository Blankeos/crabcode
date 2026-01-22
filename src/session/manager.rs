use crate::session::types::Session;
use std::collections::HashMap;
use std::time::SystemTime;

#[derive(Debug)]
pub struct SessionInfo {
    pub id: String,
    pub created_at: SystemTime,
    pub message_count: usize,
}

pub struct SessionManager {
    sessions: HashMap<String, Session>,
    current_session_id: Option<String>,
    session_counter: usize,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            current_session_id: None,
            session_counter: 0,
        }
    }

    pub fn create_session(&mut self, name: Option<String>) -> String {
        self.session_counter += 1;
        let session_id = name.unwrap_or_else(|| format!("session-{}", self.session_counter));

        let session = Session::new();
        self.sessions.insert(session_id.clone(), session);
        self.current_session_id = Some(session_id.clone());

        session_id
    }

    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        self.sessions
            .iter()
            .map(|(id, session)| SessionInfo {
                id: id.clone(),
                created_at: SystemTime::now(),
                message_count: session.messages.len(),
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

    pub fn delete_session(&mut self, id: &str) -> bool {
        if self.sessions.remove(id).is_some() {
            if self.current_session_id.as_ref() == Some(&id.to_string()) {
                self.current_session_id = None;
            }
            true
        } else {
            false
        }
    }
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
        assert_eq!(id, "session-1");
        assert!(manager.sessions.contains_key(&id));
        assert_eq!(manager.current_session_id, Some(id));
    }

    #[test]
    fn test_create_session_custom_name() {
        let mut manager = SessionManager::new();
        let id = manager.create_session(Some("my-session".to_string()));
        assert_eq!(id, "my-session");
        assert!(manager.sessions.contains_key(&id));
        assert_eq!(manager.current_session_id, Some(id));
    }

    #[test]
    fn test_create_multiple_sessions() {
        let mut manager = SessionManager::new();
        let id1 = manager.create_session(None);
        let id2 = manager.create_session(None);
        let id3 = manager.create_session(None);

        assert_eq!(id1, "session-1");
        assert_eq!(id2, "session-2");
        assert_eq!(id3, "session-3");
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
        manager.create_session(Some("test".to_string()));
        assert!(manager.get_session("test").is_some());
        assert!(manager.get_session("nonexistent").is_none());
    }

    #[test]
    fn test_switch_session() {
        let mut manager = SessionManager::new();
        manager.create_session(Some("session-1".to_string()));
        manager.create_session(Some("session-2".to_string()));

        assert!(manager.switch_session("session-1"));
        assert_eq!(manager.current_session_id, Some("session-1".to_string()));

        assert!(manager.switch_session("session-2"));
        assert_eq!(manager.current_session_id, Some("session-2".to_string()));

        assert!(!manager.switch_session("nonexistent"));
    }

    #[test]
    fn test_delete_session() {
        let mut manager = SessionManager::new();
        manager.create_session(Some("session-1".to_string()));
        manager.create_session(Some("session-2".to_string()));

        assert!(manager.delete_session("session-1"));
        assert!(!manager.sessions.contains_key("session-1"));
        assert!(manager.sessions.contains_key("session-2"));
    }

    #[test]
    fn test_delete_current_session() {
        let mut manager = SessionManager::new();
        manager.create_session(Some("session-1".to_string()));
        manager.create_session(Some("session-2".to_string()));

        manager.switch_session("session-1");
        assert!(manager.delete_session("session-1"));
        assert!(manager.current_session_id.is_none());
    }
}
