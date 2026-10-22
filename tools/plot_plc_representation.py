#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Plot measured finite-grid headroom and its perceptual tradeoffs."""
import argparse
import json
from pathlib import Path
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('report',type=Path)
    p.add_argument('output',type=Path)
    a=p.parse_args()
    data=json.loads(a.report.read_text())
    variants=['current','gain_env','gain_mse','phase_env','joint_env','joint_mse']
    labels=['Installed','Gain env','Gain MSE','Phase env','Joint env','Joint MSE']
    colors={'baseline':'#787e85','phase_env':'#2368ad','joint_env':'#15977f','joint_mse':'#bd573b'}
    fig,axes=plt.subplots(3,2,figsize=(12,11),layout='constrained')
    for col,lang in enumerate(('en','sv')):
        ax=axes[0,col]
        for v in ('baseline','phase_env','joint_env','joint_mse'):
            bands=data['correlations'][lang][v]['bands']
            last=13 if lang=='sv' else 16
            med=[b['after_q'][1] if b['after_q'] else np.nan for b in bands[:last]]
            ax.plot(range(last),med,marker='o',markersize=3,label=v.replace('_',' '),color=colors[v])
        ax.axvline(6.5,color='#555',linestyle=':',linewidth=1)
        ax.axhline(0,color='#aaa',linewidth=.6)
        ax.set(title=('English' if lang=='en' else 'Swedish')+' — median MDCT correlation',xlabel='Native band (0–6 below 1.6 kHz)',ylabel='Median rho before gains',ylim=(-.05,1))
        ax.legend(fontsize=8)
        ax=axes[1,col];x=np.arange(len(variants))
        metrics=[data['comparisons'][v]['baseline'][lang] for v in variants]
        ax.bar(x-.18,[(m['lost_error_ratio']-1)*100 for m in metrics],.36,color='#2368ad',label='Lost waveform error')
        ax.bar(x+.18,[(m['full_envelope_ratio']-1)*100 for m in metrics],.36,color='#bd573b',label='Full-band envelope error')
        ax.axhline(0,color='#555',linewidth=.8)
        ax.set(xticks=x,xticklabels=labels,ylabel='Change vs classical PLC (%)',title='Error changes — lower is better')
        ax.tick_params(axis='x',labelrotation=25)
        ax.legend(fontsize=8)
        ax=axes[2,col]
        for i,m in enumerate(metrics):
            value=m['estoi_delta'];lo,hi=m['estoi_group_bootstrap95']
            # Plot bounds separately: percentile CI need not contain the point.
            ax.plot([i,i],[lo,hi],color='#2368ad',linewidth=2)
            ax.scatter(i,value,color='#2368ad',s=25)
        ax.axhline(0,color='#555',linewidth=.8)
        ax.set(xticks=x,xticklabels=labels,ylabel='ESTOI delta vs classical PLC',title='Intelligibility — higher is better; group 95% CI')
        ax.tick_params(axis='x',labelrotation=25)
    fig.suptitle('Native PLC representation oracle: fixed phase grid + shaped noise + 16 gains\nKnown Step2 cohort; noncausal diagnostic, no model adoption',fontsize=14)
    fig.savefig(a.output,dpi=160)


if __name__=='__main__':
    main()
