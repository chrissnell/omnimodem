! Reference driver for the WSJT-X 77-bit message UNpacker: links the unmodified
! packjt77 and prints unpack77's decoded message for a 77-char 0/1 string arg.
program unpack77_dump
  use packjt77
  character*77 :: c77
  character*37 :: msg
  character*90 :: arg
  logical :: ok
  integer :: i
  call getarg(1, arg)
  do i = 1, 77; c77(i:i) = arg(i:i); end do
  call unpack77(c77, 0, msg, ok)
  print '(A)', trim(msg)
end program
