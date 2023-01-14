use actix_web::{
    web::{self, Form},
    HttpResponse, ResponseError,
};
use chrono::Utc;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    domain::{NewSubscriber, SubscriberEmail, SubscriberName, SubscriptionToken},
    email_client::EmailClient,
    startup::ApplicationBaseUrl,
};

#[derive(serde::Deserialize)]
pub struct FormData {
    email: String,
    name: String,
}

impl TryFrom<FormData> for NewSubscriber {
    type Error = String;

    fn try_from(value: FormData) -> Result<Self, Self::Error> {
        let name = SubscriberName::parse(value.name)?;
        let email = SubscriberEmail::parse(value.email)?;
        Ok(Self { email, name })
    }
}

#[tracing::instrument(
    name = "Adding a new subscriber",
    skip(form, pool, email_client, base_url),
    fields(
        subscriber_email = %form.email,
        subscriber_name = %form.name
    )
)]
pub async fn subscribe(
    form: Form<FormData>,
    pool: web::Data<PgPool>,
    email_client: web::Data<EmailClient>,
    base_url: web::Data<ApplicationBaseUrl>,
) -> Result<HttpResponse, actix_web::Error> {
    let new_subscriber = match form.0.try_into() {
        Ok(subscriber) => subscriber,
        Err(_) => return Ok(HttpResponse::BadRequest().finish()),
    };

    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(_) => return Ok(HttpResponse::InternalServerError().finish()),
    };

    let pending_subscription_token =
        check_and_get_token_pending_confirmation(&mut tx, &new_subscriber).await?;

    let subscription_token = match pending_subscription_token {
        Some(pending_token) => match SubscriptionToken::parse(pending_token) {
            Ok(token) => token,
            Err(_) => return Ok(HttpResponse::InternalServerError().finish()),
        },
        None => {
            let subscriber_id = match insert_subscriber(&mut tx, &new_subscriber).await {
                Ok(subscriber_id) => subscriber_id,
                Err(_) => return Ok(HttpResponse::InternalServerError().finish()),
            };

            let subscription_token = SubscriptionToken::generate();

            store_token(&mut tx, subscriber_id, subscription_token.as_ref()).await?;
            subscription_token
        }
    };

    if (tx.commit().await).is_err() {
        return Ok(HttpResponse::InternalServerError().finish());
    }

    if send_confirmation_email(
        &email_client,
        new_subscriber,
        &base_url.0,
        subscription_token.as_ref(),
    )
    .await
    .is_err()
    {
        return Ok(HttpResponse::InternalServerError().finish());
    }

    Ok(HttpResponse::Ok().finish())
}

#[tracing::instrument(
    name = "Checking for an existing pending subscriber and get its token",
    skip(tx, new_subscriber)
)]
async fn check_and_get_token_pending_confirmation(
    tx: &mut Transaction<'_, Postgres>,
    new_subscriber: &NewSubscriber,
) -> Result<Option<String>, GetTokenError> {
    let token = sqlx::query!(
        r#"
        SELECT subscription_token FROM subscription_tokens
        join subscriptions on subscriptions.id = subscription_tokens.subscriber_id
        WHERE subscriptions.email = $1
        AND subscriptions.status = 'pending_confirmation'
        "#,
        new_subscriber.email.as_ref()
    )
    .fetch_optional(tx)
    .await
    .map_err(|e| {
        tracing::error!("Failed to execute query: {:?}", e);
        GetTokenError(e)
    })?
    .map(|row| row.subscription_token);

    Ok(token)
}

fn error_chain_fmt(
    e: &impl std::error::Error,
    f: &mut std::fmt::Formatter<'_>,
) -> std::fmt::Result {
    writeln!(f, "{}\n", e)?;
    let mut current = e.source();
    while let Some(cause) = current {
        writeln!(f, "Caused by:\n\t{}", cause)?;
        current = cause.source();
    }
    Ok(())
}

pub struct GetTokenError(sqlx::Error);

impl ResponseError for GetTokenError {}

impl std::fmt::Display for GetTokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "A database error occurred while getting the token")
    }
}

impl std::fmt::Debug for GetTokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        error_chain_fmt(self, f)
    }
}

impl std::error::Error for GetTokenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

#[tracing::instrument(
    name = "Storing subscription token in the database",
    skip(subscription_token, tx)
)]
async fn store_token(
    tx: &mut Transaction<'_, Postgres>,
    subscriber_id: Uuid,
    subscription_token: &str,
) -> Result<(), StoreTokenError> {
    sqlx::query!(
        r#"insert into subscription_tokens (subscription_token, subscriber_id) values ($1, $2)"#,
        subscription_token,
        subscriber_id
    )
    .execute(tx)
    .await
    .map_err(|error| {
        tracing::error!("Failed to execute query: {:?}", error);
        StoreTokenError(error)
    })?;
    Ok(())
}

pub struct StoreTokenError(sqlx::Error);

impl ResponseError for StoreTokenError {}

impl std::fmt::Display for StoreTokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "A database error occurred while storing the token")
    }
}

impl std::fmt::Debug for StoreTokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        error_chain_fmt(self, f)
    }
}

impl std::error::Error for StoreTokenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

#[tracing::instrument(
    name = "Send a confirmation email to a new subscriber",
    skip(email_client, new_subscriber, base_url, subscription_token)
)]
async fn send_confirmation_email(
    email_client: &EmailClient,
    new_subscriber: NewSubscriber,
    base_url: &str,
    subscription_token: &str,
) -> Result<(), reqwest::Error> {
    let confirmation_link = format!(
        "{}/subscriptions/confirm?subscription_token={}",
        base_url, subscription_token
    );
    let plain_body = format!(
        "Welcome to our newsletter!\nVisit {} to confirm your subscription.",
        confirmation_link
    );
    let html_body = format!(
        "Welcome to our newsletter!<br/>\
             Click <a href=\"{}\">here</a> to confirm your subscription.",
        confirmation_link
    );
    email_client
        .send_email(
            new_subscriber.email,
            "Welcome!",
            plain_body.as_str(),
            html_body.as_str(),
        )
        .await
}

#[tracing::instrument(
    name = "Saving new subscriber details in the database",
    skip(new_subscriber, tx)
)]
pub async fn insert_subscriber(
    tx: &mut Transaction<'_, Postgres>,
    new_subscriber: &NewSubscriber,
) -> Result<Uuid, sqlx::Error> {
    let subscriber_id = Uuid::new_v4();
    sqlx::query!(
        r#"
        insert into subscriptions (id, email, name, subscribed_at, status)
        values ($1, $2, $3, $4, 'pending_confirmation')
        "#,
        subscriber_id,
        new_subscriber.email.as_ref(),
        new_subscriber.name.as_ref(),
        Utc::now()
    )
    .execute(tx)
    .await
    .map_err(|e| {
        tracing::error!("Failed to execute query: {:?}", e);
        e
    })?;
    Ok(subscriber_id)
}
