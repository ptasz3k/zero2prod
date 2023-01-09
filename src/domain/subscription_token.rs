use rand::{distributions::Alphanumeric, Rng};

const TOKEN_LENGTH: usize = 25;

#[derive(Debug)]
pub struct SubscriptionToken {
    token: String,
}

impl SubscriptionToken {
    pub fn generate() -> Self {
        let mut rng = rand::thread_rng();
        let token = std::iter::repeat_with(|| rng.sample(Alphanumeric))
            .map(char::from)
            .take(TOKEN_LENGTH)
            .collect();
        Self { token }
    }

    pub fn parse(token: String) -> Result<Self, String> {
        if token.len() != TOKEN_LENGTH {
            return Err("Invalid token length.".to_string());
        };

        if !token.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err("Invalid character in token.".to_string());
        }

        Ok(Self { token })
    }
}

impl AsRef<str> for SubscriptionToken {
    fn as_ref(&self) -> &str {
        &self.token
    }
}

#[cfg(test)]
mod tests {
    use claim::assert_err;

    use crate::domain::subscription_token;

    use super::SubscriptionToken;

    #[derive(Debug, Clone)]
    struct ValidTokenFixture(String);

    impl quickcheck::Arbitrary for ValidTokenFixture {
        fn arbitrary<G: quickcheck::Gen>(_g: &mut G) -> Self {
            let token = SubscriptionToken::generate();
            Self(token.as_ref().to_string())
        }
    }

    #[test]
    fn generated_token_is_of_correct_length_and_alphanumeric() {
        let token = subscription_token::SubscriptionToken::generate();
        assert_eq!(token.token.len(), subscription_token::TOKEN_LENGTH);
        assert!(token.token.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[quickcheck_macros::quickcheck]
    fn parse_valid_token_is_ok(fixture: ValidTokenFixture) -> bool {
        subscription_token::SubscriptionToken::parse(fixture.0).is_ok()
    }

    #[test]
    fn parse_invalid_token_length_is_err() {
        let token = subscription_token::SubscriptionToken::parse(
            "a".repeat(subscription_token::TOKEN_LENGTH - 1),
        );
        assert_err!(token);
    }

    #[test]
    fn parse_invalid_token_character_is_err() {
        let token = subscription_token::SubscriptionToken::parse(
            "a".repeat(subscription_token::TOKEN_LENGTH - 1) + "!",
        );
        assert_err!(token);
    }
}
