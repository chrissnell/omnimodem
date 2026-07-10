//! KISS↔gRPC bridge. `codec` is pure framing (no I/O); `listener` is the tokio
//! TCP bridge that turns a packet channel into a KISS TNC. Lives on the async
//! control edge — it only uses the public Command/FrameEvent spine, never the
//! synchronous DSP core.
pub mod codec;
pub mod listener;
