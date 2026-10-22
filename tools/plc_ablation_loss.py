# SPDX-License-Identifier: MIT
"""Applied-gain losses and exact native inverse/120-sample OLA for offline QAT."""
import numpy as np
import torch
from train_neural_plc import Predictor

HZ = np.array([0,200,400,600,800,1000,1300,1600,2000,2500,3200,4000,5200,6800,9000,13000,24000])


class AppliedPredictor(Predictor):
    def __init__(self, minimum, maximum):
        super().__init__()
        self.center = (minimum+maximum)/2
        self.radius = (maximum-minimum)/2
        # All experiments start with exactly identity gain, including 2D.
        torch.nn.init.constant_(self.layers[2].bias, float(np.arctanh((1-self.center)/self.radius)))

    def forward(self, x, qat=True):
        for layer in self.layers[:2]:
            x = torch.relu(layer(x, qat))
        return self.center+self.radius*self.layers[2](x, qat).tanh()


def band_terms(gain, data, coherent_bands=7, overshoot_weight=1.0):
    error = (data['pp']*gain.square()-2*data['pt']*gain+data['tt']).clamp_min(0)
    log_difference = torch.log(data['pp']*gain.square()+0.001)-torch.log(data['tt']+0.001)
    envelope = log_difference.square()*torch.where(log_difference>0, overshoot_weight, 1.0)
    mask = torch.ones_like(envelope)
    # Existing Swedish sources are native 16kHz. A native band crossing 8kHz
    # cannot be partially masked from these sufficient statistics.
    mask[data['language']==1, 13:] = 0
    spectral = (envelope*mask).sum(dim=-1)/mask.sum(dim=-1)
    return error[:,:coherent_bands].sum(dim=-1), spectral


def objective(gain, data, spec):
    coherent, envelope = band_terms(gain, data, spec['coherent_bands'], spec['overshoot_weight'])
    return coherent+0.01*(gain-1).square().mean(dim=-1)+0.1*envelope


def synthesize(coefficients, initial_overlap, basis):
    """coefficients [batch,frames,N], basis [N,N+120], output [batch,frames*N]."""
    blocks = coefficients@basis
    head = blocks[:,:,:-120]
    overlap = torch.cat((initial_overlap[:,None,:], blocks[:,:-1,-120:]),dim=1)
    values = torch.cat((head[:,:,:120]+overlap, head[:,:,120:]),dim=-1)
    return values.clamp(-1,1).flatten(1)


def sequence_prediction(model, data, basis, qat=True):
    x, prediction = data['x'], data['prediction']
    gain = model(x.flatten(0,1),qat).reshape(*x.shape[:2],16)
    eligible = (x[:,:,78]>=1)&(x[:,:,76]<0.985)&(x[:,:,73]<x[:,:,77])&(prediction.abs().sum(-1)>0)
    gain = torch.where(eligible[:,:,None],gain,1.0)
    n = prediction.shape[-1]
    boundaries = HZ*n//24000
    indices = np.repeat(np.arange(16),np.diff(boundaries))
    bins = torch.as_tensor(indices,device=gain.device)
    corrected = prediction*gain[:,:,bins]
    coefficients = torch.cat((corrected,data['recovery'][:,None,:]),dim=1)
    return synthesize(coefficients,data['overlap'],basis)


def mrstft(actual, target, native_rate=48000):
    losses=[]
    for n in (128,256,512):
        window=torch.hann_window(n,device=actual.device)
        a=torch.stft(actual,n,hop_length=n//4,window=window,return_complex=True).abs()
        t=torch.stft(target,n,hop_length=n//4,window=window,return_complex=True).abs()
        if native_rate==16000:
            count=int(8000*n/48000)+1
            a,t=a[:,:count],t[:,:count]
        # Per-sequence amplitude normalization prevents loud clips dominating.
        scale=t.mean(dim=(-2,-1),keepdim=True).clamp_min(1e-5)
        a,t=a/scale,t/scale
        losses.append((a-t).abs().mean((-2,-1))+(torch.log(a+0.01)-torch.log(t+0.01)).abs().mean((-2,-1)))
    return torch.stack(losses).mean(0)
