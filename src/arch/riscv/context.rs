#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct ContextData {
    // pc / sp
    pub ra: usize,
    pub sp: usize,
    // callee saved registers
    pub s: [usize; 12],
}

impl ContextData {
    pub fn new(ra: usize, sp: usize) -> Self {
        Self {
            ra: ra,
            sp: sp,
            ..ContextData::default()
        }
    }
}
