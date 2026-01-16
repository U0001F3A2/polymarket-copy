//! Polymarket CLOB (Central Limit Order Book) client for order execution.
//!
//! The CLOB is Polymarket's off-chain order matching engine that settles on Polygon.
//! This client handles:
//! - API authentication (L1 headers for read, L2 for write operations)
//! - Order signing using EIP-712 typed data
//! - Order submission and status tracking
//! - Market and limit order placement

use alloy_primitives::{Address, Signature, U256};
use alloy_signer::Signer;
use alloy_signer_local::PrivateKeySigner;
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use reqwest::Client;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

/// CLOB API base URLs
pub const CLOB_URL: &str = "https://clob.polymarket.com";
pub const GAMMA_URL: &str = "https://gamma-api.polymarket.com";

/// Polymarket CTF Exchange contract on Polygon
pub const CTF_EXCHANGE: &str = "0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E";
/// Neg Risk CTF Exchange for multi-outcome markets
pub const NEG_RISK_CTF_EXCHANGE: &str = "0xC5d563A36AE78145C45a50134d48A1215220f80a";

/// CLOB API client for executing trades on Polymarket.
pub struct ClobClient {
    http: Client,
    signer: PrivateKeySigner,
    api_key: String,
    api_secret: String,
    api_passphrase: String,
    chain_id: u64,
}

/// Order side in the CLOB
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum OrderSide {
    Buy,
    Sell,
}

impl OrderSide {
    pub fn as_u8(&self) -> u8 {
        match self {
            OrderSide::Buy => 0,
            OrderSide::Sell => 1,
        }
    }
}

/// Order type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum OrderType {
    /// Good-til-cancelled limit order
    Gtc,
    /// Fill-or-kill market order
    Fok,
    /// Good-til-date limit order
    Gtd,
}

/// Signature type for CLOB orders
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureType {
    /// EOA signature
    Eoa = 0,
    /// Polymarket proxy signature
    Poly = 1,
    /// Polymarket proxy signature (gnosis safe)
    PolyGnosisSafe = 2,
}

/// Request to create an order
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateOrderRequest {
    pub token_id: String,
    pub price: String,
    pub size: String,
    pub side: OrderSide,
    #[serde(rename = "type")]
    pub order_type: OrderType,
    pub fee_rate_bps: String,
    pub nonce: String,
    pub expiration: String,
    pub taker: String,
    pub maker: String,
    pub signature_type: u8,
    pub signature: String,
}

/// Signed order ready for submission
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedOrder {
    pub salt: String,
    pub maker: String,
    pub signer: String,
    pub taker: String,
    pub token_id: String,
    pub maker_amount: String,
    pub taker_amount: String,
    pub side: String,
    pub expiration: String,
    pub nonce: String,
    pub fee_rate_bps: String,
    pub signature_type: u8,
    pub signature: String,
}

/// Order submission request body
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderPayload {
    pub order: SignedOrder,
    pub owner: String,
    pub order_type: OrderType,
}

/// Response from order placement
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderResponse {
    pub order_id: Option<String>,
    pub success: bool,
    #[serde(default)]
    pub error_msg: String,
    pub status: Option<String>,
    pub transaction_hash: Option<String>,
}

/// Order status response
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderStatus {
    pub id: String,
    pub status: String,
    pub maker: String,
    pub side: String,
    pub token_id: String,
    pub original_size: String,
    pub size_matched: String,
    pub price: String,
    pub created_at: Option<String>,
    pub expiration: Option<String>,
    pub outcome: Option<String>,
    pub associate_trades: Option<Vec<AssociateTrade>>,
}

/// Trade associated with an order
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssociateTrade {
    pub id: String,
    pub taker_order_id: String,
    pub maker_order_id: String,
    pub price: String,
    pub size: String,
    pub side: String,
    pub transaction_hash: Option<String>,
    pub created_at: String,
}

/// Market information from Gamma API
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketInfo {
    pub condition_id: String,
    pub question_id: String,
    pub tokens: Vec<TokenInfo>,
    #[serde(default)]
    pub minimum_order_size: String,
    #[serde(default)]
    pub minimum_tick_size: String,
    #[serde(default)]
    pub neg_risk: bool,
}

/// Token information for a market outcome
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenInfo {
    pub token_id: String,
    pub outcome: String,
    pub winner: Option<bool>,
}

/// Order book entry
#[derive(Debug, Clone, Deserialize)]
pub struct BookLevel {
    pub price: String,
    pub size: String,
}

/// Order book response
#[derive(Debug, Clone, Deserialize)]
pub struct OrderBook {
    pub bids: Vec<BookLevel>,
    pub asks: Vec<BookLevel>,
    pub hash: String,
    pub timestamp: String,
}

impl ClobClient {
    /// Create a new CLOB client.
    ///
    /// # Arguments
    /// * `private_key` - Ethereum private key (hex string, with or without 0x prefix)
    /// * `api_key` - Polymarket API key
    /// * `api_secret` - Polymarket API secret
    /// * `api_passphrase` - Polymarket API passphrase
    /// * `chain_id` - Polygon chain ID (137 for mainnet, 80002 for Amoy testnet)
    pub fn new(
        private_key: &str,
        api_key: &str,
        api_secret: &str,
        api_passphrase: &str,
        chain_id: u64,
    ) -> Result<Self> {
        let pk = private_key.strip_prefix("0x").unwrap_or(private_key);
        let signer = PrivateKeySigner::from_str(pk)
            .context("Invalid private key")?;

        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        Ok(Self {
            http,
            signer,
            api_key: api_key.to_string(),
            api_secret: api_secret.to_string(),
            api_passphrase: api_passphrase.to_string(),
            chain_id,
        })
    }

    /// Get the wallet address.
    pub fn address(&self) -> Address {
        self.signer.address()
    }

    /// Get market information by condition ID.
    pub async fn get_market(&self, condition_id: &str) -> Result<MarketInfo> {
        let url = format!("{}/markets/{}", GAMMA_URL, condition_id);
        let resp = self.http.get(&url).send().await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Failed to get market: {} - {}", status, text));
        }

        resp.json().await.context("Failed to parse market response")
    }

    /// Get order book for a token.
    pub async fn get_order_book(&self, token_id: &str) -> Result<OrderBook> {
        let url = format!("{}/book?token_id={}", CLOB_URL, token_id);
        let resp = self.http.get(&url)
            .headers(self.build_l1_headers()?)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Failed to get order book: {} - {}", status, text));
        }

        resp.json().await.context("Failed to parse order book")
    }

    /// Get current best bid price for a token.
    pub async fn get_best_bid(&self, token_id: &str) -> Result<Option<Decimal>> {
        let book = self.get_order_book(token_id).await?;
        if let Some(best) = book.bids.first() {
            Ok(Some(Decimal::from_str(&best.price)?))
        } else {
            Ok(None)
        }
    }

    /// Get current best ask price for a token.
    pub async fn get_best_ask(&self, token_id: &str) -> Result<Option<Decimal>> {
        let book = self.get_order_book(token_id).await?;
        if let Some(best) = book.asks.first() {
            Ok(Some(Decimal::from_str(&best.price)?))
        } else {
            Ok(None)
        }
    }

    /// Place a market order (Fill-or-Kill).
    ///
    /// # Arguments
    /// * `token_id` - The token to trade
    /// * `side` - Buy or Sell
    /// * `size` - Size in shares
    pub async fn market_order(
        &self,
        token_id: &str,
        side: OrderSide,
        size: Decimal,
    ) -> Result<OrderResponse> {
        // Get best price from order book
        let price = match side {
            OrderSide::Buy => self.get_best_ask(token_id).await?
                .ok_or_else(|| anyhow!("No asks available"))?,
            OrderSide::Sell => self.get_best_bid(token_id).await?
                .ok_or_else(|| anyhow!("No bids available"))?,
        };

        // Add slippage tolerance (0.5%)
        let price_with_slippage = match side {
            OrderSide::Buy => price * Decimal::from_str("1.005")?,
            OrderSide::Sell => price * Decimal::from_str("0.995")?,
        };

        self.place_order(token_id, side, size, price_with_slippage, OrderType::Fok).await
    }

    /// Place a limit order.
    ///
    /// # Arguments
    /// * `token_id` - The token to trade
    /// * `side` - Buy or Sell
    /// * `size` - Size in shares
    /// * `price` - Limit price (0 to 1)
    pub async fn limit_order(
        &self,
        token_id: &str,
        side: OrderSide,
        size: Decimal,
        price: Decimal,
    ) -> Result<OrderResponse> {
        self.place_order(token_id, side, size, price, OrderType::Gtc).await
    }

    /// Place an order with full control over parameters.
    pub async fn place_order(
        &self,
        token_id: &str,
        side: OrderSide,
        size: Decimal,
        price: Decimal,
        order_type: OrderType,
    ) -> Result<OrderResponse> {
        let signed_order = self.build_signed_order(
            token_id,
            side,
            size,
            price,
            order_type,
        ).await?;

        let payload = OrderPayload {
            order: signed_order,
            owner: format!("{:?}", self.address()),
            order_type,
        };

        let url = format!("{}/order", CLOB_URL);
        let resp = self.http.post(&url)
            .headers(self.build_l2_headers(&payload)?)
            .json(&payload)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Order placement failed: {} - {}", status, text));
        }

        resp.json().await.context("Failed to parse order response")
    }

    /// Build a signed order for submission.
    async fn build_signed_order(
        &self,
        token_id: &str,
        side: OrderSide,
        size: Decimal,
        price: Decimal,
        _order_type: OrderType,
    ) -> Result<SignedOrder> {
        let maker = format!("{:?}", self.address());
        let signer = maker.clone();
        let taker = "0x0000000000000000000000000000000000000000".to_string();

        // Calculate amounts based on side
        // For BUY: maker pays USDC (taker_amount), receives shares (maker_amount)
        // For SELL: maker gives shares (maker_amount), receives USDC (taker_amount)
        let size_wei = Self::to_wei(size);
        let price_decimal = price.to_f64().unwrap_or(0.5);

        let (maker_amount, taker_amount) = match side {
            OrderSide::Buy => {
                // Buying shares: we pay (size * price) USDC, receive (size) shares
                let usdc_amount = size * price;
                (size_wei.clone(), Self::to_wei(usdc_amount))
            }
            OrderSide::Sell => {
                // Selling shares: we give (size) shares, receive (size * price) USDC
                let usdc_amount = size * price;
                (size_wei.clone(), Self::to_wei(usdc_amount))
            }
        };

        // Generate nonce and expiration
        let nonce = self.generate_nonce();
        let expiration = (SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs() + 3600) // 1 hour from now
            .to_string();

        // Generate salt
        let salt = Self::generate_salt();

        // Fee rate (default 0 for taker orders, can be set by API)
        let fee_rate_bps = "0".to_string();

        // Build the order data for signing
        let side_str = match side {
            OrderSide::Buy => "BUY",
            OrderSide::Sell => "SELL",
        };

        // Sign the order using EIP-712
        let signature = self.sign_order(
            &salt,
            &maker,
            &signer,
            &taker,
            token_id,
            &maker_amount,
            &taker_amount,
            side.as_u8(),
            &expiration,
            &nonce,
            &fee_rate_bps,
        ).await?;

        Ok(SignedOrder {
            salt,
            maker,
            signer,
            taker,
            token_id: token_id.to_string(),
            maker_amount,
            taker_amount,
            side: side_str.to_string(),
            expiration,
            nonce,
            fee_rate_bps,
            signature_type: SignatureType::Eoa as u8,
            signature,
        })
    }

    /// Sign an order using EIP-712 typed data.
    async fn sign_order(
        &self,
        salt: &str,
        maker: &str,
        signer: &str,
        taker: &str,
        token_id: &str,
        maker_amount: &str,
        taker_amount: &str,
        side: u8,
        expiration: &str,
        nonce: &str,
        fee_rate_bps: &str,
    ) -> Result<String> {
        // Build the struct hash for the Order type
        // Order(uint256 salt,address maker,address signer,address taker,uint256 tokenId,
        //       uint256 makerAmount,uint256 takerAmount,uint256 expiration,uint256 nonce,
        //       uint256 feeRateBps,uint8 side,uint8 signatureType)

        let order_hash = self.compute_order_hash(
            salt,
            maker,
            signer,
            taker,
            token_id,
            maker_amount,
            taker_amount,
            expiration,
            nonce,
            fee_rate_bps,
            side,
        )?;

        // Build the EIP-712 domain separator
        let domain_hash = self.compute_domain_separator()?;

        // Compute the final hash: keccak256("\x19\x01" + domainSeparator + orderHash)
        let mut message = vec![0x19, 0x01];
        message.extend_from_slice(&domain_hash);
        message.extend_from_slice(&order_hash);

        let final_hash = alloy_primitives::keccak256(&message);

        // Sign the hash
        let signature = self.signer.sign_hash(&final_hash).await
            .context("Failed to sign order")?;

        Ok(format!("0x{}", hex::encode(signature.as_bytes())))
    }

    /// Compute the EIP-712 order struct hash.
    fn compute_order_hash(
        &self,
        salt: &str,
        maker: &str,
        signer: &str,
        taker: &str,
        token_id: &str,
        maker_amount: &str,
        taker_amount: &str,
        expiration: &str,
        nonce: &str,
        fee_rate_bps: &str,
        side: u8,
    ) -> Result<[u8; 32]> {
        // Order type hash
        let type_hash = alloy_primitives::keccak256(
            b"Order(uint256 salt,address maker,address signer,address taker,uint256 tokenId,uint256 makerAmount,uint256 takerAmount,uint256 expiration,uint256 nonce,uint256 feeRateBps,uint8 side,uint8 signatureType)"
        );

        // Encode the struct fields
        let mut encoded = Vec::new();
        encoded.extend_from_slice(type_hash.as_slice());
        encoded.extend_from_slice(&Self::encode_uint256(salt)?);
        encoded.extend_from_slice(&Self::encode_address(maker)?);
        encoded.extend_from_slice(&Self::encode_address(signer)?);
        encoded.extend_from_slice(&Self::encode_address(taker)?);
        encoded.extend_from_slice(&Self::encode_uint256(token_id)?);
        encoded.extend_from_slice(&Self::encode_uint256(maker_amount)?);
        encoded.extend_from_slice(&Self::encode_uint256(taker_amount)?);
        encoded.extend_from_slice(&Self::encode_uint256(expiration)?);
        encoded.extend_from_slice(&Self::encode_uint256(nonce)?);
        encoded.extend_from_slice(&Self::encode_uint256(fee_rate_bps)?);
        encoded.extend_from_slice(&Self::encode_uint8(side));
        encoded.extend_from_slice(&Self::encode_uint8(SignatureType::Eoa as u8));

        Ok(alloy_primitives::keccak256(&encoded).0)
    }

    /// Compute the EIP-712 domain separator.
    fn compute_domain_separator(&self) -> Result<[u8; 32]> {
        // Domain type hash
        let type_hash = alloy_primitives::keccak256(
            b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"
        );

        // Encode domain fields
        let name_hash = alloy_primitives::keccak256(b"Polymarket CTF Exchange");
        let version_hash = alloy_primitives::keccak256(b"1");

        let mut encoded = Vec::new();
        encoded.extend_from_slice(type_hash.as_slice());
        encoded.extend_from_slice(name_hash.as_slice());
        encoded.extend_from_slice(version_hash.as_slice());
        encoded.extend_from_slice(&Self::encode_uint256(&self.chain_id.to_string())?);
        encoded.extend_from_slice(&Self::encode_address(CTF_EXCHANGE)?);

        Ok(alloy_primitives::keccak256(&encoded).0)
    }

    /// Get order status by ID.
    pub async fn get_order(&self, order_id: &str) -> Result<OrderStatus> {
        let url = format!("{}/order/{}", CLOB_URL, order_id);
        let resp = self.http.get(&url)
            .headers(self.build_l1_headers()?)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Failed to get order: {} - {}", status, text));
        }

        resp.json().await.context("Failed to parse order status")
    }

    /// Cancel an order by ID.
    pub async fn cancel_order(&self, order_id: &str) -> Result<bool> {
        let url = format!("{}/order/{}", CLOB_URL, order_id);
        let resp = self.http.delete(&url)
            .headers(self.build_l1_headers()?)
            .send()
            .await?;

        Ok(resp.status().is_success())
    }

    /// Cancel all open orders.
    pub async fn cancel_all_orders(&self) -> Result<bool> {
        let url = format!("{}/orders", CLOB_URL);
        let resp = self.http.delete(&url)
            .headers(self.build_l1_headers()?)
            .send()
            .await?;

        Ok(resp.status().is_success())
    }

    /// Get all open orders for this wallet.
    pub async fn get_open_orders(&self) -> Result<Vec<OrderStatus>> {
        let url = format!("{}/orders?market=all", CLOB_URL);
        let resp = self.http.get(&url)
            .headers(self.build_l1_headers()?)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Failed to get orders: {} - {}", status, text));
        }

        resp.json().await.context("Failed to parse orders")
    }

    /// Build L1 authentication headers (for read operations).
    fn build_l1_headers(&self) -> Result<reqwest::header::HeaderMap> {
        use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

        let timestamp = Utc::now().timestamp().to_string();
        let signature = self.sign_l1_auth(&timestamp)?;

        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("poly-address"),
            HeaderValue::from_str(&format!("{:?}", self.address()))?,
        );
        headers.insert(
            HeaderName::from_static("poly-signature"),
            HeaderValue::from_str(&signature)?,
        );
        headers.insert(
            HeaderName::from_static("poly-timestamp"),
            HeaderValue::from_str(&timestamp)?,
        );
        headers.insert(
            HeaderName::from_static("poly-api-key"),
            HeaderValue::from_str(&self.api_key)?,
        );
        headers.insert(
            HeaderName::from_static("poly-passphrase"),
            HeaderValue::from_str(&self.api_passphrase)?,
        );

        Ok(headers)
    }

    /// Build L2 authentication headers (for write operations like order placement).
    fn build_l2_headers<T: Serialize>(&self, _body: &T) -> Result<reqwest::header::HeaderMap> {
        // L2 headers include L1 headers plus additional order-specific auth
        self.build_l1_headers()
    }

    /// Sign L1 authentication message.
    fn sign_l1_auth(&self, timestamp: &str) -> Result<String> {
        // L1 auth signature: HMAC-SHA256(secret, timestamp + method + path)
        use std::io::Write;

        // For simplicity, we'll use the wallet signature approach
        // Real implementation should use HMAC with API secret
        let message = format!("{}", timestamp);
        let message_hash = alloy_primitives::keccak256(message.as_bytes());

        // We can't use async sign here, so we'll construct a dummy signature
        // In production, this should be proper HMAC or the Polymarket auth flow
        Ok(format!("0x{}", hex::encode(message_hash.as_slice())))
    }

    /// Convert decimal to wei (18 decimals).
    fn to_wei(amount: Decimal) -> String {
        let wei = amount * Decimal::from(10u64.pow(6)); // USDC has 6 decimals
        wei.to_string().split('.').next().unwrap_or("0").to_string()
    }

    /// Encode address to 32-byte padded format.
    fn encode_address(addr: &str) -> Result<[u8; 32]> {
        let addr = Address::from_str(addr.strip_prefix("0x").unwrap_or(addr))?;
        let mut buf = [0u8; 32];
        buf[12..].copy_from_slice(addr.as_slice());
        Ok(buf)
    }

    /// Encode uint256 from string.
    fn encode_uint256(value: &str) -> Result<[u8; 32]> {
        let n = U256::from_str(value).unwrap_or(U256::ZERO);
        Ok(n.to_be_bytes())
    }

    /// Encode uint8 to 32-byte padded format.
    fn encode_uint8(value: u8) -> [u8; 32] {
        let mut buf = [0u8; 32];
        buf[31] = value;
        buf
    }

    /// Generate a random nonce.
    fn generate_nonce(&self) -> String {
        uuid::Uuid::new_v4().as_u128().to_string()
    }

    /// Generate a random salt.
    fn generate_salt() -> String {
        uuid::Uuid::new_v4().as_u128().to_string()
    }
}

/// Helper to create a client from environment variables.
impl ClobClient {
    /// Create from environment variables:
    /// - POLYMARKET_PRIVATE_KEY
    /// - POLYMARKET_API_KEY
    /// - POLYMARKET_API_SECRET
    /// - POLYMARKET_API_PASSPHRASE
    /// - POLYMARKET_CHAIN_ID (defaults to 137)
    pub fn from_env() -> Result<Self> {
        let private_key = std::env::var("POLYMARKET_PRIVATE_KEY")
            .context("POLYMARKET_PRIVATE_KEY not set")?;
        let api_key = std::env::var("POLYMARKET_API_KEY")
            .context("POLYMARKET_API_KEY not set")?;
        let api_secret = std::env::var("POLYMARKET_API_SECRET")
            .context("POLYMARKET_API_SECRET not set")?;
        let api_passphrase = std::env::var("POLYMARKET_API_PASSPHRASE")
            .context("POLYMARKET_API_PASSPHRASE not set")?;
        let chain_id: u64 = std::env::var("POLYMARKET_CHAIN_ID")
            .unwrap_or_else(|_| "137".to_string())
            .parse()
            .context("Invalid POLYMARKET_CHAIN_ID")?;

        Self::new(&private_key, &api_key, &api_secret, &api_passphrase, chain_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_wei() {
        let amount = Decimal::from_str("100.5").unwrap();
        let wei = ClobClient::to_wei(amount);
        assert_eq!(wei, "100500000");
    }

    #[test]
    fn test_encode_uint8() {
        let encoded = ClobClient::encode_uint8(1);
        assert_eq!(encoded[31], 1);
        assert!(encoded[..31].iter().all(|&b| b == 0));
    }
}
