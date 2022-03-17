# executor的入口函数
#
#   void executor_entry()
#
# 该函数只是简单的获取栈底的 executor 实例地址
# 然后再跳转到 runtime.rs::run_executor 函数。
#
  .section .text
  .global executor_entry
  .balign 4
executor_entry:
  popfq
  pop rdi
  jmp run_executor


