! Reference driver for the FST4/WSJT-X 77-bit message packer: links the
! UNMODIFIED packjt77 and prints pack77's 77-bit c77 (0/1 string) for a message
! given on the command line. KATs the Rust pack77 (standard Type-1) port.
program pack77_dump
  use packjt77
  character*37 :: msg0
  character*77 :: c77
  integer :: i3, n3, i
  call getarg(1, msg0)
  i3 = -1; n3 = -1
  call pack77(msg0, i3, n3, c77)
  do i = 1, 77; write(*,'(A1)',advance='no') c77(i:i); end do
  write(*,*)
end program
