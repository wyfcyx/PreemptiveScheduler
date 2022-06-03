#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct ContextData {
    // pc / sp
    pub lr: usize,
    pub sp: usize,
    // callee saved registers
    pub s: [usize; 18],
    // pg base register
    pub ttbr0: usize,
}

impl ContextData {
    pub fn new(lr: usize, sp: usize, ttbr0: usize) -> Self {
        Self {
            lr,
            sp,
            ttbr0,
            ..ContextData::default()
        }
    }
}
