! Stub of genmsk40 (short-message MSK40 generator) so genmsk_128_90.f90 links
! standalone. Our golden messages never take the '<...>' short-message path,
! so this is never called; it only satisfies the linker.
subroutine genmsk40(msg,msgsent,ichk,itone,itype)
  character*37 msg,msgsent; integer ichk,itype; integer*4 itone(144)
  itype=-1; msgsent=' '; itone=0
end subroutine genmsk40
