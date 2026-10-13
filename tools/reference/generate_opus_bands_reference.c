// SPDX-License-Identifier: MIT
// Black-box caller for the unchanged external RFC 6716 Appendix A oracle.
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include "bands.h"
#include "entdec.h"
#include "modes.h"
static void u32(uint32_t x) { unsigned char b[4]={x,x>>8,x>>16,x>>24}; fwrite(b,1,4,stdout); }
static int collapse_oracle(const CELTMode *mode) {
  float x[1920],current[42],previous[42],older[42]; unsigned char mask[42]; int p[21];
  for(int lm=0;lm<4;lm++) for(int c=1;c<=2;c++) {
    for(int i=0;i<1920;i++) x[i]=(float)((i*17)%31-15)/50;
    for(int b=0;b<21;b++) {
      p[b]=b*29;
      for(int ch=0;ch<2;ch++) {
        current[ch*21+b]=(float)(b-10)/4;
        previous[ch*21+b]=(float)(b-12-ch)/8;
        older[ch*21+b]=(float)(b-9+ch)/16;
      }
      for(int ch=0;ch<c;ch++) mask[b*c+ch]=(unsigned char)((b*13+ch*37)&((1<<lm)-1));
    }
    anti_collapse(mode,x,mask,lm,c,960,0,21,current,previous,older,p,UINT32_C(0xabcdef01));
    u32(lm);u32(c);
    for(int ch=0;ch<c;ch++) for(int i=0;i<(100<<lm);i++) {
      uint32_t bits; memcpy(&bits,&x[ch*960+i],4);u32(bits);
    }
  }
  return 0;
}
int main(int argc, char **argv) {
  int error; const CELTMode *mode=opus_custom_mode_create(48000,960,&error); if(!mode) return 2;
  if(argc==2 && strcmp(argv[1],"anti-collapse")==0) return collapse_oracle(mode);
  unsigned char packet[4000], mask[42]; float x[960], y[960], e[42]; int p[21],tf[21];
  const int budgets[3]={0,72,360};
  const int narrow[21]={31,26,26,24,21,19,17,14,25,23,16,18,25};
  for(int lm=0;lm<4;lm++) for(int t=0;t<2;t++) for(int c=1;c<=2;c++) for(int spread=0;spread<4;spread++) for(int choice=0;choice<4;choice++) {
    memset(x,0,sizeof x); memset(y,0,sizeof y); memset(mask,0,sizeof mask);
    for(int b=0;b<21;b++) { p[b]=choice==3?narrow[b]:budgets[choice]; tf[b]=choice==3?0:lm>0?(t&&b%3==0?lm:(b%2==0?-1:0)):0; e[b]=e[b+21]=1; }
    for(int k=0;k<4000;k++) packet[k]=(unsigned char)(k*73+lm*17+t*31+c*49+spread*11+choice*29);
    ec_dec decoder; ec_dec_init(&decoder,packet,sizeof packet); uint32_t seed=0;
    int end=choice==3?13:21;
    quant_all_bands(0,mode,0,end,x,c==2?y:NULL,mask,e,p,t?(1<<lm):0,spread,c==2&&choice==1,choice==0?0:choice==1?15:choice==3?6:21,tf,4000*64,0,&decoder,lm,end,&seed);
    u32(lm); u32(t); u32(c);u32(spread);u32(choice);u32(decoder.rng);u32(ec_tell_frac(&decoder));u32(seed);u32(decoder.error);
    fwrite(mask,1,42,stdout);
    for(int ch=0;ch<c;ch++) for(int k=0;k<(100<<lm);k++) {uint32_t bits;float v=ch?y[k]:x[k];memcpy(&bits,&v,4);u32(bits);}
  }
}
