
use super::nsfw_head_vectors::{BIAS, WEIGHTS};

const _: () = assert!(WEIGHTS.len() == super::vision::DIM);

pub fn probability(embedding: &[f32]) -> f32 {
    let z = super::vision::dot(embedding, &WEIGHTS) + BIAS;
    1.0 / (1.0 + (-z).exp())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_probability_is_a_probability() {
        let along = super::super::vision::unit(&WEIGHTS);
        let against: Vec<f32> = along.iter().map(|x| -x).collect();
        let (p_along, p_against) = (probability(&along), probability(&against));
        assert!((0.0..=1.0).contains(&p_along) && (0.0..=1.0).contains(&p_against));
        assert!(
            p_along > p_against,
            "the direction of the weights is the direction of the offence"
        );
        let zero = vec![0.0; WEIGHTS.len()];
        assert!(
            (probability(&zero) - 1.0 / (1.0 + (-BIAS).exp())).abs() < 1e-6,
            "an empty embedding answers with the bias alone"
        );
    }

    #[test]
    fn the_prior_is_ordinary() {
        const {
            assert!(
                BIAS < 0.0,
                "with no evidence at all the head must not lean towards deleting"
            )
        };
    }
}
