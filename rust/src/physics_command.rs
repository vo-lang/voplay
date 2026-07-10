use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PhysicsCommandError {
    pub(crate) code: &'static str,
    pub(crate) offset: usize,
    pub(crate) opcode: Option<u8>,
    pub(crate) needed: usize,
    pub(crate) remaining: usize,
}

impl fmt::Display for PhysicsCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "voplay.physics.command_error code={} offset={} opcode={} needed={} remaining={}",
            self.code,
            self.offset,
            self.opcode
                .map(|opcode| opcode.to_string())
                .unwrap_or_else(|| "none".to_string()),
            self.needed,
            self.remaining,
        )
    }
}

pub(crate) struct PhysicsCommandReader<'a> {
    data: &'a [u8],
    position: usize,
    command_offset: usize,
}

impl<'a> PhysicsCommandReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            position: 0,
            command_offset: 0,
        }
    }

    pub(crate) fn next_header(&mut self) -> Result<Option<(u8, u32)>, PhysicsCommandError> {
        if self.position == self.data.len() {
            return Ok(None);
        }
        self.command_offset = self.position;
        self.require(5, None, "truncated_header")?;
        let opcode = self.data[self.position];
        self.position += 1;
        let body_id = u32::from_le_bytes([
            self.data[self.position],
            self.data[self.position + 1],
            self.data[self.position + 2],
            self.data[self.position + 3],
        ]);
        self.position += 4;
        Ok(Some((opcode, body_id)))
    }

    pub(crate) fn read_f64(
        &mut self,
        opcode: u8,
        code: &'static str,
    ) -> Result<f64, PhysicsCommandError> {
        self.require(8, Some(opcode), code)?;
        let bytes = [
            self.data[self.position],
            self.data[self.position + 1],
            self.data[self.position + 2],
            self.data[self.position + 3],
            self.data[self.position + 4],
            self.data[self.position + 5],
            self.data[self.position + 6],
            self.data[self.position + 7],
        ];
        self.position += 8;
        Ok(f64::from_le_bytes(bytes))
    }

    pub(crate) fn unknown_opcode(&self, opcode: u8) -> PhysicsCommandError {
        PhysicsCommandError {
            code: "unknown_opcode",
            offset: self.command_offset,
            opcode: Some(opcode),
            needed: 0,
            remaining: self.data.len().saturating_sub(self.position),
        }
    }

    fn require(
        &self,
        needed: usize,
        opcode: Option<u8>,
        code: &'static str,
    ) -> Result<(), PhysicsCommandError> {
        let remaining = self.data.len().saturating_sub(self.position);
        if remaining < needed {
            return Err(PhysicsCommandError {
                code,
                offset: self.command_offset,
                opcode,
                needed,
                remaining,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_command_offset_for_truncated_payload() {
        let mut reader = PhysicsCommandReader::new(&[3, 7, 0, 0, 0, 1, 2]);
        let (opcode, _) = reader.next_header().expect("header").expect("command");
        let error = reader
            .read_f64(opcode, "truncated_value")
            .expect_err("payload must be rejected");
        assert_eq!(error.offset, 0);
        assert_eq!(error.opcode, Some(3));
        assert_eq!(error.remaining, 2);
        assert!(error.to_string().contains("code=truncated_value"));
    }
}
