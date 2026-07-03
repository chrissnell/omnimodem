! Reference driver for the FST4 24-bit CRC (poly 0x100065b): links the
! UNMODIFIED get_crc24.f90 and prints ncrc24 for a fixed 101-bit array whose
! first 77 bits are a test pattern and last 24 are zero. KATs the Rust port.
program fst4_crc24_dump
  integer*1 :: mc(101)
  integer :: ncrc, i
  do i = 1, 101
     if (i <= 77) then
        mc(i) = merge(1_1, 0_1, mod(i-1, 4) < 2)   ! 1,1,0,0 repeating
     else
        mc(i) = 0
     end if
  end do
  call get_crc24(mc, 101, ncrc)
  print '(I0)', ncrc
end program
