use crate::AgentError;
use serde::{Deserialize, Serialize};

pub const PROCESS_GROUP_OWNER_ARGUMENT: &str = "--process-group-owner";

/// 由原生调用方装配的外部进程组归属，不属于模型请求或持久配置。
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessExecutionContext {
    pub owner_process_group: u32,
}

impl ProcessExecutionContext {
    /// 仅验证描述范围；实际组身份和组长存活由执行器逐次核验。
    pub fn validate(self) -> Result<(), AgentError> {
        if self.owner_process_group > 1 && self.owner_process_group <= i32::MAX as u32 {
            Ok(())
        } else {
            Err(AgentError::ConfigurationInvalid)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 外部进程组描述拒绝危险范围和未知字段() {
        for owner_process_group in [0, 1, u32::MAX] {
            assert!(
                ProcessExecutionContext {
                    owner_process_group
                }
                .validate()
                .is_err()
            );
        }
        let context: ProcessExecutionContext =
            serde_json::from_str(r#"{"ownerProcessGroup":42}"#).unwrap();
        assert!(context.validate().is_ok());
        assert!(
            serde_json::from_str::<ProcessExecutionContext>(
                r#"{"ownerProcessGroup":42,"command":"unexpected"}"#
            )
            .is_err()
        );
    }
}
