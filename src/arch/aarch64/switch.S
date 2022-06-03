# Context switch
#
#   void swtch(struct ContextData *old, struct ContextData *new);
#
# Save current registers in old. Load from new.

    .text
    .globl switch
    .balign 4
switch:
        stp x28, x29, [a0, #-16]!
        stp x26, x27, [a0, #-16]!
        stp x24, x25, [a0, #-16]!
        stp x22, x23, [a0, #-16]!
        stp x20, x21, [a0, #-16]!
        stp x15, x19, [a0, #-16]!
        stp x13, x14, [a0, #-16]!
        stp x11, x12, [a0, #-16]!
        stp x9, x10, [a0, #-16]!
        stp lr, sp, [a0, #-16]!

        mrs x9, ttbr0_el1
        str x9, [a0], #-16

        ldr x9, [a1], #112
        lsr x10, x9, #12
        msr x9, ttbr0_el1
        dsb ishst
        tlbi vaae1is, x10
        dsb ish
        isb

        ldp lr, sp, [a1], #16
        ldp x9, x10, [a1], #16
        ldp x11, x12, [a1], #16
        ldp x13, x14, [a1], #16
        ldp x15, x19, [a1], #16
        ldp x20, x21, [a1], #16
        ldp x22, x23, [a1], #16
        ldp x24, x25, [a1], #16
        ldp x26, x27, [a1], #16
        ldp x28, x29, [a1], #16

        ret