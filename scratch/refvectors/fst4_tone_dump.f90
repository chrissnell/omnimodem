! Reference FST4 tone-sequence generator: links the UNMODIFIED genfst4.f90 and
! calls its get_fst4_tones_from_bits entry for a fixed 101-bit msgbits, printing
! the 160 tone indices (0..3). Used to KAT the Rust fst4 tone assembly.
program fst4_tone_dump
  integer*1 :: msgbits(101)
  integer*4 :: i4tone(160)
  integer :: i, iwspr
  do i = 1, 101; msgbits(i) = merge(1_1, 0_1, mod(i-1,3) == 0); end do
  iwspr = 0
  call get_fst4_tones_from_bits(msgbits, i4tone, iwspr)
  do i = 1, 160; write(*,'(I1)',advance='no') i4tone(i); end do
  write(*,*)
end program
