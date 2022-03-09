#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct ContextData {
    // callee saved registers
    pub r15: usize,
    pub r14: usize,
    pub r13: usize,
    pub r12: usize,
    pub rbp: usize,
    pub rbx: usize,
    // pc
    pub rip: usize,
}

impl ContextData {
    pub fn new(rip: usize, _sp: usize) -> Self {
        Self {
            rip: rip, // TODO
            ..ContextData::default()
        }
    }
}
