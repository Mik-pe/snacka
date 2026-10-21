#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Plot measured waveform outcomes and energy-supported band correlations."""
import argparse
import json
from pathlib import Path
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('summary',type=Path);p.add_argument('output',type=Path);a=p.parse_args()
    report=json.loads(a.summary.read_text());assert report['status']=='COMPLETE'
    variants=['current','oracle_mdct','oracle_env','oracle_combined','oracle_wide']
    labels=['MLP i main','MDCT-orakel\nbegränsat','Envelopp-orakel\nbegränsat','Kombinationsorakel\nbegränsat','Kombinationsorakel\nbrett']
    fig=plt.figure(figsize=(13,9),layout='constrained');grid=fig.add_gridspec(2,2,height_ratios=[1.1,1]);upper=fig.add_subplot(grid[0,:])
    x=np.arange(len(variants));width=.34
    for index,(language,title,color) in enumerate([('en','Engelska','#2e6f9e'),('sv','Svenska','#dc8535')]):
        rows=[next(r for r in report['waveforms'] if r['language']==language and r['implementation']==v) for v in variants]
        values=np.array([r['error_energy_reduction_fraction']*100 for r in rows]);ci=np.array([[100*(1-10**(-d/10)) for d in r['group_bootstrap_95pct_db']] for r in rows])
        position=x+(index-.5)*width
        bars=upper.bar(position,values,width,label=title,color=color)
        upper.errorbar(position,ci.mean(axis=1),yerr=(ci[:,1]-ci[:,0])*.5,fmt='none',ecolor='#333',capsize=3,linewidth=1)
        upper.bar_label(bars,labels=[f'{v:.1f}%' for v in values],padding=4,fontsize=9)
    upper.set_xticks(x,labels);upper.set_ylabel('Minskning av felenergi i tappat ljud (%)');upper.axhline(0,color='#777',linewidth=.8)
    upper.set_title('Faktiskt avkodat ljud mot samma PLC-baseline',loc='left',fontweight='bold');upper.legend(frameon=False,ncols=2)
    upper.grid(axis='y',alpha=.2);upper.set_axisbelow(True);upper.margins(y=.22)
    for column,(language,title) in enumerate([('en','Engelska'),('sv','Svenska')]):
        ax=fig.add_subplot(grid[1,column])
        for condition,label,color in [('clean','Rent','#2e6f9e'),('noise20','Brus 20 dB','#dc8535'),('noise10','Brus 10 dB','#8e5c94')]:
            rows=sorted([r for r in report['correlations'] if r['language']==language and r['scope']=='runtime_eligible' and r['condition']==condition],key=lambda r:r['band'])
            hz=np.array([sum(r['hz'])/2 for r in rows]);q=np.array([r['supported_rho_q25_q50_q75'] or [np.nan]*3 for r in rows])
            ax.plot(hz,q[:,1],'-o',markersize=3,color=color,label=label);ax.fill_between(hz,q[:,0],q[:,2],color=color,alpha=.12)
        ax.set_xscale('log');ax.set_xlim(100,20000);ax.set_ylim(-1,1);ax.axhline(0,color='#777',linestyle='--',linewidth=.8)
        ticks=[200,500,1000,2000,4000,8000,16000];ax.set_xticks(ticks,['200','500','1k','2k','4k','8k','16k']);ax.set_xlabel('Bandcentrum (Hz)');ax.set_ylabel('MDCT-korrelation ρ')
        ax.set_title(title+' – median och kvartiler',loc='left');ax.grid(alpha=.15);ax.legend(frameon=False,fontsize=9)
        if language=='sv':ax.axvline(8000,color='#777',linestyle=':',linewidth=1)
    fig.suptitle('Snacka fas 2: orakeldiagnostik',fontsize=17,fontweight='bold')
    fig.supxlabel('Oraklen använder framtida facit. Felstaplar: 95 % gruppbootstrap. Korrelation: band med relativ energi >10⁻⁶ i båda signalerna.',fontsize=9)
    fig.savefig(a.output,dpi=180);plt.close(fig)


if __name__=='__main__':main()
