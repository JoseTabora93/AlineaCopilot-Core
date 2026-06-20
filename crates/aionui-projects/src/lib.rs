//! Orquestación del módulo Proyectos (Alinea Fase 2 #2).
//!
//! Hogar de la lógica de orquestación supervisor/ejecutor — fuera del DB layer y
//! del HTTP layer. Hoy: el **motor de handoffs** (cascada de dependencias). En
//! slices 6-7 albergará la instanciación de pipelines desde plantillas (Hermes)
//! y el dispatch a agentes (OpenClaw).

mod handoff;
mod pipeline;

pub use handoff::{HandoffEngine, HandoffOutcome};
pub use pipeline::{PipelineError, PipelineService};
