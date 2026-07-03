! Reference FST4 GFSK waveform generator: links the UNMODIFIED gen_fst4wave +
! gfsk_pulse and prints the real audio samples for a small fixed case
! (nsym=4, nsps=16, itone=0,1,2,3), used to FP-tolerance-KAT the Rust port.
program fst4_wave_dump
  integer, parameter :: nsym=4, nsps=16, nwave=(nsym+2)*nsps
  integer :: itone(nsym), hmod, icmplx, i
  real :: wave(nwave), fsample, f0
  complex :: cwave(nwave)
  itone = (/0,1,2,3/)
  fsample = 12000.0; hmod = 1; f0 = 1500.0; icmplx = 0
  call gen_fst4wave(itone,nsym,nsps,nwave,fsample,hmod,f0,icmplx,cwave,wave)
  do i = 1, nwave; write(*,'(F0.8)') wave(i); end do
end program
