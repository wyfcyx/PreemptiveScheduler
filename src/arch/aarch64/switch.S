# Context switch
#
#   void swtch(struct ContextData *old, struct ContextData *new);
#
# Save current registers in old. Load from new.

    .text
    .globl switch
    .balign 4
switch:
        stp x28, x29, [x0, #-16]!
        stp x26, x27, [x0, #-16]!
        stp x24, x25, [x0, #-16]!
        stp x22, x23, [x0, #-16]!
        stp x20, x21, [x0, #-16]!
        stp x15, x19, [x0, #-16]!
        stp x13, x14, [x0, #-16]!
        stp x11, x12, [x0, #-16]!
        stp x9, x10, [x0, #-16]!
        stp lr, x31, [x0, #-16]!

        mrs x9, ttbr0_el1
        str x9, [x0], #-16

        ldr x9, [x1], #112
        lsr x10, x9, #12
        msr ttbr0_el1, x9
        dsb ishst
        tlbi vaae1is, x10
        dsb ish
        isb

        ldp lr, x31, [x1], #16
        ldp x9, x10, [x1], #16
        ldp x11, x12, [x1], #16
        ldp x13, x14, [x1], #16
        ldp x15, x19, [x1], #16
        ldp x20, x21, [x1], #16
        ldp x22, x23, [x1], #16
        ldp x24, x25, [x1], #16
        ldp x26, x27, [x1], #16
        ldp x28, x29, [x1], #16

        ret