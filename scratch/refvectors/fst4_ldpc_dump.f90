! Reference golden-vector generator for the FST4/FST4W LDPC codes.
! Links the UNMODIFIED wsjtx encoders encode240_101 / encode240_74 (which
! include ldpc_240_{101,74}_generator.f90) and prints, for a fixed message,
! the systematic 240-bit codeword as a 0/1 string. Used to KAT the Rust port.
!
! Build (from omnimodem/): see scratch/refvectors/build_fst4_ldpc.sh
program fst4_ldpc_dump
  implicit none
  integer*1 :: m101(101), cw101(240)
  integer*1 :: m74(74), cw74(240)
  integer :: i
  ! Deterministic test messages: 1,0,0,1 repeating (payload+CRC bit domain).
  do i = 1, 101
     m101(i) = merge(1_1, 0_1, mod(i-1, 3) == 0)
  end do
  do i = 1, 74
     m74(i) = merge(1_1, 0_1, mod(i-1, 3) == 0)
  end do
  call encode240_101(m101, cw101)
  call encode240_74(m74, cw74)
  write(*,'(A)') '240_101_msg'
  do i = 1, 101; write(*,'(I1)',advance='no') m101(i); end do; write(*,*)
  write(*,'(A)') '240_101_codeword'
  do i = 1, 240; write(*,'(I1)',advance='no') cw101(i); end do; write(*,*)
  write(*,'(A)') '240_74_msg'
  do i = 1, 74; write(*,'(I1)',advance='no') m74(i); end do; write(*,*)
  write(*,'(A)') '240_74_codeword'
  do i = 1, 240; write(*,'(I1)',advance='no') cw74(i); end do; write(*,*)
end program fst4_ldpc_dump
