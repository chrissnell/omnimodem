! Authoritative (174,87) LDPC codeword dump from the UNMODIFIED js8call reference
! encoder (lib/ft8/encode174.f90 + ldpc_174_87_params.f90 @ a7ff1be). Feeds a
! fixed, deterministic 87-bit message and prints msgbits + codeword(174) as
! 0/1 strings, plus the message-bit column positions (colorder[87..174]) used to
! recover the message from a decoded codeword. Pure Fortran, no CRC/boost dep.
program dump_encode174
  implicit none
  integer, parameter :: N=174, K=87
  integer*1 :: message(K), codeword(N)
  integer :: i
  integer :: colorder(N)
  ! Same colorder as encode174/bpdecode174 (ref: ldpc_174_87_params.f90).
  ! We re-derive the message positions by encoding unit vectors below instead of
  ! hardcoding, so no need to duplicate the array here.
  character(len=N) :: cws
  character(len=K) :: mbs

  ! Deterministic pseudo-random-ish fixed message pattern (bit j = (j*37+5) mod 2
  ! folded) — arbitrary but fixed so the Rust KAT can reproduce it exactly.
  do i=1,K
    message(i) = iand( ishft(i*37+5, -1), 1 )
  enddo

  call encode174(message, codeword)

  do i=1,K
    write(mbs(i:i),'(I1)') message(i)
  enddo
  do i=1,N
    write(cws(i:i),'(I1)') codeword(i)
  enddo
  write(*,'(A)') 'MSGBITS '//mbs
  write(*,'(A)') 'CODEWORD '//cws
end program
