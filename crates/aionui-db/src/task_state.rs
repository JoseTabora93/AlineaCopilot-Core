//! Máquina de estados de tareas (Alinea Fase 2 #2 — slice 4).
//!
//! Lógica PURA: define las transiciones legales. La persistencia vive en
//! `ITaskRepository`; la cascada de handoffs por dependencias y el dispatch a
//! agentes llegan en slices 5-7.

/// Estados de una tarea.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Todo,
    InProgress,
    InReview,
    Done,
    Rejected,
    Blocked,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Todo => "todo",
            TaskStatus::InProgress => "in_progress",
            TaskStatus::InReview => "in_review",
            TaskStatus::Done => "done",
            TaskStatus::Rejected => "rejected",
            TaskStatus::Blocked => "blocked",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "todo" => TaskStatus::Todo,
            "in_progress" => TaskStatus::InProgress,
            "in_review" => TaskStatus::InReview,
            "done" => TaskStatus::Done,
            "rejected" => TaskStatus::Rejected,
            "blocked" => TaskStatus::Blocked,
            _ => return None,
        })
    }
}

/// Acciones que disparan transiciones de estado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskAction {
    Start,
    Submit,
    Approve,
    Reject,
    Reopen,
}

impl TaskAction {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "start" => TaskAction::Start,
            "submit" => TaskAction::Submit,
            "approve" => TaskAction::Approve,
            "reject" => TaskAction::Reject,
            "reopen" => TaskAction::Reopen,
            _ => return None,
        })
    }
}

/// Estado resultante de aplicar `action` a `from`, o `None` si la transición es
/// ilegal. `requires_review` decide si `submit` va a `InReview` o directo a `Done`.
pub fn next_status(from: TaskStatus, action: TaskAction, requires_review: bool) -> Option<TaskStatus> {
    use TaskAction::*;
    use TaskStatus::*;
    match (from, action) {
        // Arranque (incluye desbloqueo manual de una tarea blocked).
        (Todo, Start) | (Blocked, Start) => Some(InProgress),
        // Entrega: con gate → revisión; sin gate → hecho.
        (InProgress, Submit) => Some(if requires_review { InReview } else { Done }),
        // Revisión humana.
        (InReview, Approve) => Some(Done),
        (InReview, Reject) => Some(Rejected),
        // Reapertura de una tarea rechazada.
        (Rejected, Reopen) => Some(Todo),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legal_transitions() {
        assert_eq!(
            next_status(TaskStatus::Todo, TaskAction::Start, true),
            Some(TaskStatus::InProgress)
        );
        // Con gate, submit → in_review; sin gate → done.
        assert_eq!(
            next_status(TaskStatus::InProgress, TaskAction::Submit, true),
            Some(TaskStatus::InReview)
        );
        assert_eq!(
            next_status(TaskStatus::InProgress, TaskAction::Submit, false),
            Some(TaskStatus::Done)
        );
        assert_eq!(
            next_status(TaskStatus::InReview, TaskAction::Approve, true),
            Some(TaskStatus::Done)
        );
        assert_eq!(
            next_status(TaskStatus::InReview, TaskAction::Reject, true),
            Some(TaskStatus::Rejected)
        );
        assert_eq!(
            next_status(TaskStatus::Rejected, TaskAction::Reopen, true),
            Some(TaskStatus::Todo)
        );
        assert_eq!(
            next_status(TaskStatus::Blocked, TaskAction::Start, true),
            Some(TaskStatus::InProgress)
        );
    }

    #[test]
    fn illegal_transitions_rejected() {
        // No se puede aprobar algo que no está en revisión.
        assert_eq!(next_status(TaskStatus::Todo, TaskAction::Approve, true), None);
        // No se puede entregar algo que no está en progreso.
        assert_eq!(next_status(TaskStatus::Todo, TaskAction::Submit, true), None);
        // No se puede re-trabajar algo ya hecho.
        assert_eq!(next_status(TaskStatus::Done, TaskAction::Start, true), None);
        assert_eq!(next_status(TaskStatus::Done, TaskAction::Reject, true), None);
    }

    #[test]
    fn status_roundtrip() {
        for s in ["todo", "in_progress", "in_review", "done", "rejected", "blocked"] {
            assert_eq!(TaskStatus::parse(s).unwrap().as_str(), s);
        }
        assert!(TaskStatus::parse("bogus").is_none());
        assert!(TaskAction::parse("bogus").is_none());
    }
}
