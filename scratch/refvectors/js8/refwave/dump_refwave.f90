! Reference JS8 waveform generator for the omnimodem cross-decode gate (Phase W5).
! Given 87 msgbits + submode params, runs the UNMODIFIED reference encoder
! (encode174.f90) + the genjs8 tone assembly + the genjs8refsig continuous-phase
! signal model, and writes real audio samples (i16) that our Rust Js8Demod must
! decode. Pure gfortran — no Qt/boost/FFTW. ref: js8call/lib/js8/{genjs8,
! genjs8refsig}.f90 @ a7ff1be.
!   argv: <87-bit string> <NSPS> <icos:1|2> <f0Hz> <outfile>
program dump_refwave
  implicit none
  integer, parameter :: N=174, K=87, ND=58, NN=79
  integer*1 :: message(K), codeword(N)
  integer :: itone(NN), icos7a(0:6), icos7b(0:6), icos7c(0:6)
  integer :: i, j, k2, indx, nsps, icos, nz, is
  real*8  :: f0, twopi, phi, dphi, dt, xnsps, s
  character(len=200) :: bitarg, a2, a3, a4, outfile
  character(len=1) :: ch
  integer*2, allocatable :: iwave(:)

  call get_command_argument(1, bitarg)
  call get_command_argument(2, a2); read(a2,*) nsps
  call get_command_argument(3, a3); read(a3,*) icos
  call get_command_argument(4, a4); read(a4,*) f0
  call get_command_argument(5, outfile)

  do i=1,K
    ch = bitarg(i:i)
    message(i) = ichar(ch) - ichar('0')
  enddo

  call encode174(message, codeword)

  if(icos.eq.1) then
    icos7a = (/4,2,5,6,1,3,0/); icos7b = (/4,2,5,6,1,3,0/); icos7c = (/4,2,5,6,1,3,0/)
  else
    icos7a = (/0,6,2,3,5,4,1/); icos7b = (/1,5,0,2,3,6,4/); icos7c = (/2,5,0,6,4,1,3/)
  endif
  itone(1:7)=icos7a
  itone(36+1:36+7)=icos7b
  itone(NN-6:NN)=icos7c
  k2=7
  do j=1,ND
    i=3*j-2
    k2=k2+1
    if(j.eq.30) k2=k2+7
    indx=codeword(i)*4 + codeword(i+1)*2 + codeword(i+2)
    itone(k2)=indx
  enddo

  ! genjs8refsig continuous-phase model, real part -> i16 audio.
  nz = NN*nsps
  allocate(iwave(nz))
  twopi = 8.d0*atan(1.d0)
  xnsps = nsps*1.0d0
  dt = 1.d0/12000.d0
  phi = 0.d0
  k2 = 0
  do i=1,NN
    dphi = twopi*(f0*dt + itone(i)/xnsps)
    do is=1,nsps
      k2 = k2+1
      s = cos(phi)
      iwave(k2) = nint(0.5d0 * 32767.d0 * s)
      phi = mod(phi+dphi, twopi)
    enddo
  enddo

  open(unit=10, file=trim(outfile), access='stream', form='unformatted', status='replace')
  write(10) iwave
  close(10)
  write(*,'(A,I0,A)') 'wrote ', nz, ' i16 samples to '//trim(outfile)
end program
