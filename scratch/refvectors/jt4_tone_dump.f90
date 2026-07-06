! Reference JT4 channel-symbol generator: links the UNMODIFIED wsjtx bit-domain
! encode path (entail + encode232 K=32 Fano code + interleave4 + the npr sync
! vector, exactly as gen4.f90 assembles them) and prints, for a FIXED 72-bit
! message payload, first the 72 payload bits then the 206 4-FSK channel-symbol
! values (0..3). The payload is fed directly as twelve 6-bit words so the vector
! isolates the FEC/interleave/sync stages from packjt's message packing (whose
! exact 72-bit layout is the #[ignore] cross-decode gate, not this KAT). Used to
! KAT the Rust jt4 TX assembly bit-for-bit.
program jt4_tone_dump
  use jt4
  integer :: dgen(13)
  integer*1 :: data0(13), symbol(216)
  integer :: itone(206)
  integer :: i, j, n
  ! Fixed 6-bit payload words spanning the full 0..63 range.
  dgen = (/ 63, 0, 42, 21, 58, 5, 17, 36, 60, 9, 48, 27, 0 /)

  ! 72 payload bits, MSB-first within each 6-bit word (matches entail's order).
  do i = 1, 12
     n = dgen(i)
     do j = 5, 0, -1
        write(*,'(I1)',advance='no') iand(ishft(n,-j),1)
     end do
  end do
  write(*,*)

  call entail(dgen, data0)
  call encode232(data0, 206, symbol)     ! K=32, r=1/2 convolutional encoding
  call interleave4(symbol, 1)            ! forward JT4 interleave
  do i = 1, 206
     itone(i) = 2*symbol(i) + npr(i+1)   ! data = MSB, sync (npr(2:)) = LSB
  end do
  do i = 1, 206; write(*,'(I1)',advance='no') itone(i); end do
  write(*,*)
end program jt4_tone_dump
