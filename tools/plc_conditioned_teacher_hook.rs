    /// Export future labels while advancing only the installed baseline state.
    pub fn conceal_conditioned_teacher(
        &mut self,
        output: &mut [f32],
        target: &[f32],
        record: &mut [f32; 227],
    ) -> Result<FrameInfo, Error> {
        if self.finished {
            return Err(Error::Finished);
        }
        let samples = self.duration.samples();
        if output.len() != samples || target.len() != samples {
            return Err(Error::Output);
        }
        self.concealment.begin();
        let start = (samples - ALGORITHMIC_DELAY) / 2;
        let input = self
            .plc_history
            .input(samples, self.concealment.features(samples));
        let eligible =
            self.neural_plc && input[78] >= 1.0 && input[76] < 0.985 && input[73] < input[77];
        let hidden = if eligible {
            crate::plc_conditioned::latent(&input)
        } else {
            [0.; 64]
        };
        let (gains, phase) = if eligible {
            crate::plc_conditioned::base_controls(&hidden)
        } else {
            ([1.; 16], 0.)
        };
        let mut candidates = Vec::new();
        for &delta in crate::representation::PHASES {
            self.block.fill(0.);
            for (ahead, value) in self.block[start..start + samples + ALGORITHMIC_DELAY]
                .iter_mut()
                .enumerate()
            {
                *value = self.concealment.phase_sample(ahead, delta);
            }
            let mut c = vec![0.; samples];
            self.plan
                .forward(&self.block[..2 * samples], &mut c, &mut self.workspace)
                .map_err(|_| Error::Packet)?;
            candidates.push(c);
        }
        candidates.push(candidates[0].clone());
        let (_, wide) = crate::representation::select(
            &candidates,
            target,
            &input,
            crate::representation::Kind::PhaseWide,
            16,
            20261022,
        );
        let index = crate::representation::PHASES
            .iter()
            .position(|&p| p == wide.phase)
            .ok_or(Error::Packet)?;
        record[..80].copy_from_slice(&input);
        record[80..144].copy_from_slice(&hidden);
        record[144..160].copy_from_slice(&self.plc_history.last_log_power());
        record[176..192].copy_from_slice(&crate::plc_conditioned::powers(target));
        record[192..208].copy_from_slice(&crate::plc_conditioned::powers(&candidates[index]));
        record[208..224].copy_from_slice(&gains);
        record[224] = phase;
        record[226] = (input[74] * 6.).round() + 1.;
        self.block.fill(0.);
        for (ahead, value) in self.block[start..start + samples + ALGORITHMIC_DELAY]
            .iter_mut()
            .enumerate()
        {
            *value = self.concealment.phase_sample(ahead, phase);
        }
        let mut coefficients = [0.; FRAME_SAMPLES];
        self.plan
            .forward(
                &self.block[..2 * samples],
                &mut coefficients[..samples],
                &mut self.workspace,
            )
            .map_err(|_| Error::Packet)?;
        record[160..176].copy_from_slice(&crate::plc_conditioned::powers(&coefficients[..samples]));
        record[225] = if eligible && coefficients[..samples].iter().any(|&v| v != 0.) {
            1.
        } else {
            0.
        };
        if eligible {
            let edges = crate::neural_plc::edges(samples);
            for b in 0..16 {
                for value in &mut coefficients[edges[b]..edges[b + 1]] {
                    *value *= gains[b];
                }
            }
        }
        self.synthesize(&coefficients, output)?;
        self.concealment.advance(samples);
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);
        Ok(FrameInfo {
            sequence,
            bytes: 0,
            concealed: true,
            finished: false,
            valid_samples: samples,
        })
    }
