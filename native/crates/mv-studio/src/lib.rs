//! Backend for Studio, the narration pipeline: Python discovery, the JSON-lines
//! worker bridge (`python-bridge.ts` port), a sequential job queue for OCR, text
//! enhancement, narration and TTS/video pipeline jobs, and the TTS voice catalog
//! with preview helpers. No UI.

pub mod bridge;
pub mod error;
pub mod jobs;
pub mod python;
pub mod tts;

#[cfg(test)]
mod testutil;

pub use bridge::{run_worker, run_worker_logged, CancelToken, WorkerMessage, WorkerReport, WorkerSpec};
pub use error::StudioError;
pub use jobs::{
    CombineJob, EnhanceJob, JobEvent, JobPayload, JobQueue, JobSnapshot, JobState, NarrateJob,
    PipelineJob, ProviderConfig, TextExtractJob,
};
pub use python::{discover_python, PythonInterpreter, PythonLayout};
