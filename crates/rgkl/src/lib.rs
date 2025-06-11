// Copyright 2024-2025 Andres Morey
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! RGKL (RipGrep for Kubernetes Logs) - High-performance log searching library
//! 
//! This library provides efficient log searching and streaming capabilities
//! specifically designed for Kubernetes container logs. It can be used as
//! a standalone library or through the CLI binary.

pub mod error;
pub mod stream_backward;
pub mod stream_forward;
pub mod util;
pub mod z;

// Re-export commonly used types and functions
pub use error::Error;
pub use stream_forward::FollowFrom;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_library_exports() {
        // Test that main exports are available
        use crate::stream_forward::FollowFrom;
        let _follow_from = FollowFrom::Default;
    }
} 