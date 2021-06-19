// Copyright (c) Facebook, Inc. and its affiliates.
use serde::{Deserialize, Serialize};

use rd_util::*;

static CMD_ACK_DOC: &str = "\
//
// rd-agent command ack file
//
// When the commands in cmd.rs are accepted for processing, its cmd_seq is
// copied to this file. This can be used to synchronize command issuing.
//
";

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CmdAck {
    pub cmd_seq: u64,
}

impl Default for CmdAck {
    fn default() -> Self {
        Self { cmd_seq: 0 }
    }
}

impl JsonLoad for CmdAck {}

impl JsonSave for CmdAck {
    fn preamble() -> Option<String> {
        Some(CMD_ACK_DOC.into())
    }
}
