#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Plot native low-band error versus ESTOI; these are objective proxies."""
import argparse
import json
from pathlib import Path
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('report',type=Path);p.add_argument('output',type=Path)
    a=p.parse_args();report=json.loads(a.report.read_text())
    rows=report['validation_native']['rows']
    fresh=report.get('fresh_acceptance',{}).get('by_language',[])
    fig,axes=plt.subplots(1,2,figsize=(10.4,4.5),sharex=True,sharey=True)
    offsets={'control':(5,-13),'2A':(5,8),'2B':(-22,-14),'2C':(5,0),'2D':(6,-12)}
    for ax,language,title in zip(axes,('en','sv'),('English','Swedish')):
        ax.axhline(0,color='#404040',linewidth=.8);ax.axvline(0,color='#aaaaaa',linewidth=.6)
        for row in rows:
            if row['language']!=language or row['variant']=='installed':continue
            x=-row['lowband_error_change_pct'];y=row['estoi_delta_vs_installed'];name=row['variant']
            ax.scatter(x,y,s=38,color='#276B9B',zorder=3)
            offset=offsets[name]
            if language=='sv':offset={'2A':(12,-18),'2B':(12,17),'2C':(38,2)}.get(name,offset)
            ax.annotate(name,(x,y),xytext=offset,textcoords='offset points',fontsize=9,
                        arrowprops={'arrowstyle':'-','linewidth':.5,'color':'#888888'})
        for row in fresh:
            if row['language']!=language:continue
            x=100*(1-row['lowband_error_ratio']);y=row['estoi_delta']
            ax.scatter(x,y,s=130,marker='*',color='#B54D2C',zorder=4)
            interval=row.get('estoi_group_bootstrap_95pct')
            if interval:ax.plot([x,x],interval,color='#B54D2C',linewidth=1,zorder=2)
            ax.annotate('2D acceptance',(x,y),xytext=(7,6),textcoords='offset points',fontsize=9,color='#963B20')
        ax.set_title(title,fontsize=12);ax.set_xlabel('Reduction in coherent error below 1.6 kHz (%)')
        ax.grid(axis='both',alpha=.14);ax.spines[['top','right']].set_visible(False)
        ax.ticklabel_format(axis='y',style='plain',useOffset=False)
    axes[0].set_ylabel('ESTOI change relative to installed model')
    values=[r['estoi_delta_vs_installed'] for r in rows]+[r['estoi_delta'] for r in fresh]
    axes[0].set_ylim(min(-.0025,min(values)-.0003),max(.0004,max(values)+.0002))
    fig.suptitle('Less coherent error did not ensure better intelligibility',fontsize=13)
    fig.text(.5,.025,'Blue: 432 validation cases per model (6 clips/language, 24 kb/s, 20 ms).  Orange: fresh acceptance.\nHigher ESTOI is better; points below zero regress. Swedish speaker separation is unknown. No listening test.',ha='center',fontsize=8,color='#444444')
    fig.tight_layout(rect=(0,.12,1,.94));fig.savefig(a.output,dpi=170);plt.close(fig)


if __name__=='__main__':main()
