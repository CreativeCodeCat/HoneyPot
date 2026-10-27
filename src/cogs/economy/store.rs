//! Per-server economy accounts, persisted to SQLite. Every balance change runs
//! inside a transaction, so balances can't go negative or be double-spent.

use rusqlite::{params, Connection, OptionalExtension, Transaction};

const DB_FILE: &str = "database/economy.db";

/// New accounts start with this much in the bank
pub const STARTING_BANK: i64 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Balance {
    pub wallet: i64,
    pub bank: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pocket {
    Wallet,
    Bank,
}

impl Pocket {
    fn column(self) -> &'static str {
        match self {
            Pocket::Wallet => "wallet",
            Pocket::Bank => "bank",
        }
    }

    fn get(self, balance: Balance) -> i64 {
        match self {
            Pocket::Wallet => balance.wallet,
            Pocket::Bank => balance.bank,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum EcoError {
    /// The user hasn't joined the economy
    NoAccount,
    /// The other user in a payment hasn't joined the economy
    RecipientNoAccount,
    /// Not enough money; carries how much they do have
    NotEnough(i64),
    /// The balance would exceed the maximum
    TooLarge,
    /// /eco work is on cooldown until this Unix time
    OnCooldown(i64),
}

/// Outer error: database failure. Inner error: the request can't be done.
pub type EcoResult<T> = rusqlite::Result<Result<T, EcoError>>;

/// Highest balance any wallet or bank can hold
const MAX_BALANCE: i64 = 1_000_000_000_000;

pub struct Store {
    conn: Connection,
}

fn balance_in(tx: &Transaction, guild_id: u64, user_id: u64) -> rusqlite::Result<Option<Balance>> {
    tx.query_row(
        "SELECT wallet, bank FROM accounts WHERE guild_id = ? AND user_id = ?",
        [guild_id, user_id],
        |row| Ok(Balance { wallet: row.get(0)?, bank: row.get(1)? }),
    )
    .optional()
}

fn set_balance(tx: &Transaction, guild_id: u64, user_id: u64, balance: Balance) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE accounts SET wallet = ?, bank = ? WHERE guild_id = ? AND user_id = ?",
        params![balance.wallet, balance.bank, guild_id, user_id],
    )?;
    Ok(())
}

impl Store {
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_path(DB_FILE)
    }

    fn open_path(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS accounts (
                guild_id INTEGER NOT NULL,
                user_id INTEGER NOT NULL,
                wallet INTEGER NOT NULL DEFAULT 0,
                bank INTEGER NOT NULL DEFAULT 0,
                -- Unix time of the last /eco work
                last_work INTEGER,
                PRIMARY KEY (guild_id, user_id)
            )",
        )?;
        Ok(Self { conn })
    }

    /// Opens an account. Returns false if the user already has one.
    pub fn join(&self, guild_id: u64, user_id: u64) -> rusqlite::Result<bool> {
        let created = self.conn.execute(
            "INSERT OR IGNORE INTO accounts (guild_id, user_id, wallet, bank) VALUES (?, ?, 0, ?)",
            params![guild_id, user_id, STARTING_BANK],
        )?;
        Ok(created > 0)
    }

    pub fn balance(&self, guild_id: u64, user_id: u64) -> rusqlite::Result<Option<Balance>> {
        self.conn
            .query_row(
                "SELECT wallet, bank FROM accounts WHERE guild_id = ? AND user_id = ?",
                [guild_id, user_id],
                |row| Ok(Balance { wallet: row.get(0)?, bank: row.get(1)? }),
            )
            .optional()
    }

    /// Pays `wage` into the wallet if the cooldown since the last work has passed.
    pub fn work(&mut self, guild_id: u64, user_id: u64, wage: i64, now: i64, cooldown: i64) -> EcoResult<Balance> {
        let tx = self.conn.transaction()?;
        let Some(mut balance) = balance_in(&tx, guild_id, user_id)? else { return Ok(Err(EcoError::NoAccount)) };

        let last_work: Option<i64> =
            tx.query_row("SELECT last_work FROM accounts WHERE guild_id = ? AND user_id = ?", [guild_id, user_id], |row| row.get(0))?;
        if let Some(last) = last_work.filter(|last| now < last + cooldown) {
            return Ok(Err(EcoError::OnCooldown(last + cooldown)));
        }

        balance.wallet = (balance.wallet + wage).min(MAX_BALANCE);
        set_balance(&tx, guild_id, user_id, balance)?;
        tx.execute("UPDATE accounts SET last_work = ? WHERE guild_id = ? AND user_id = ?", params![now, guild_id, user_id])?;
        tx.commit()?;
        Ok(Ok(balance))
    }

    /// Moves money between a user's own wallet and bank.
    pub fn transfer(&mut self, guild_id: u64, user_id: u64, amount: i64, from: Pocket) -> EcoResult<Balance> {
        let tx = self.conn.transaction()?;
        let Some(mut balance) = balance_in(&tx, guild_id, user_id)? else { return Ok(Err(EcoError::NoAccount)) };

        let available = from.get(balance);
        if available < amount {
            return Ok(Err(EcoError::NotEnough(available)));
        }
        let (source, dest) = match from {
            Pocket::Wallet => (&mut balance.wallet, &mut balance.bank),
            Pocket::Bank => (&mut balance.bank, &mut balance.wallet),
        };
        if *dest + amount > MAX_BALANCE {
            return Ok(Err(EcoError::TooLarge));
        }
        *source -= amount;
        *dest += amount;

        set_balance(&tx, guild_id, user_id, balance)?;
        tx.commit()?;
        Ok(Ok(balance))
    }

    /// Moves money from one user's wallet to another's.
    pub fn pay(&mut self, guild_id: u64, from: u64, to: u64, amount: i64) -> EcoResult<Balance> {
        let tx = self.conn.transaction()?;
        let Some(mut sender) = balance_in(&tx, guild_id, from)? else { return Ok(Err(EcoError::NoAccount)) };
        let Some(mut recipient) = balance_in(&tx, guild_id, to)? else { return Ok(Err(EcoError::RecipientNoAccount)) };

        if sender.wallet < amount {
            return Ok(Err(EcoError::NotEnough(sender.wallet)));
        }
        if recipient.wallet + amount > MAX_BALANCE {
            return Ok(Err(EcoError::TooLarge));
        }
        sender.wallet -= amount;
        recipient.wallet += amount;

        set_balance(&tx, guild_id, from, sender)?;
        set_balance(&tx, guild_id, to, recipient)?;
        tx.commit()?;
        Ok(Ok(sender))
    }

    /// Admin adjustment: adds (positive) or removes (negative) money from a pocket.
    pub fn adjust(&mut self, guild_id: u64, user_id: u64, pocket: Pocket, delta: i64) -> EcoResult<Balance> {
        let tx = self.conn.transaction()?;
        let Some(balance) = balance_in(&tx, guild_id, user_id)? else { return Ok(Err(EcoError::NoAccount)) };

        let current = pocket.get(balance);
        let new = current + delta;
        if new < 0 {
            return Ok(Err(EcoError::NotEnough(current)));
        }
        if new > MAX_BALANCE {
            return Ok(Err(EcoError::TooLarge));
        }

        tx.execute(
            &format!("UPDATE accounts SET {} = ? WHERE guild_id = ? AND user_id = ?", pocket.column()),
            params![new, guild_id, user_id],
        )?;
        let balance = balance_in(&tx, guild_id, user_id)?.unwrap();
        tx.commit()?;
        Ok(Ok(balance))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open_path(":memory:").unwrap()
    }

    #[test]
    fn join_starts_with_bank_and_only_once() {
        let s = store();
        assert!(s.join(1, 10).unwrap());
        assert!(!s.join(1, 10).unwrap());
        assert_eq!(s.balance(1, 10).unwrap(), Some(Balance { wallet: 0, bank: STARTING_BANK }));
        // Accounts are per server
        assert_eq!(s.balance(2, 10).unwrap(), None);
    }

    #[test]
    fn deposit_and_withdraw_check_funds() {
        let mut s = store();
        s.join(1, 10).unwrap();
        assert_eq!(s.transfer(1, 10, 40, Pocket::Bank).unwrap(), Ok(Balance { wallet: 40, bank: 60 }));
        assert_eq!(s.transfer(1, 10, 41, Pocket::Wallet).unwrap(), Err(EcoError::NotEnough(40)));
        assert_eq!(s.transfer(1, 10, 40, Pocket::Wallet).unwrap(), Ok(Balance { wallet: 0, bank: 100 }));
        assert_eq!(s.transfer(1, 99, 1, Pocket::Wallet).unwrap(), Err(EcoError::NoAccount));
    }

    #[test]
    fn pay_moves_money_without_overdraft() {
        let mut s = store();
        s.join(1, 10).unwrap();
        s.join(1, 20).unwrap();
        s.transfer(1, 10, 100, Pocket::Bank).unwrap().unwrap();

        assert_eq!(s.pay(1, 10, 20, 101).unwrap(), Err(EcoError::NotEnough(100)));
        assert_eq!(s.pay(1, 10, 30, 5).unwrap(), Err(EcoError::RecipientNoAccount));
        assert_eq!(s.pay(1, 10, 20, 30).unwrap(), Ok(Balance { wallet: 70, bank: 0 }));
        assert_eq!(s.balance(1, 20).unwrap().unwrap().wallet, 30);
        // A failed payment changes nothing
        assert_eq!(s.balance(1, 10).unwrap().unwrap().wallet, 70);
    }

    #[test]
    fn work_respects_cooldown() {
        let mut s = store();
        s.join(1, 10).unwrap();
        assert_eq!(s.work(1, 10, 50, 1000, 100).unwrap(), Ok(Balance { wallet: 50, bank: 100 }));
        assert_eq!(s.work(1, 10, 50, 1050, 100).unwrap(), Err(EcoError::OnCooldown(1100)));
        assert_eq!(s.work(1, 10, 50, 1100, 100).unwrap().unwrap().wallet, 100);
    }

    #[test]
    fn adjust_never_goes_negative() {
        let mut s = store();
        s.join(1, 10).unwrap();
        assert_eq!(s.adjust(1, 10, Pocket::Wallet, 25).unwrap(), Ok(Balance { wallet: 25, bank: 100 }));
        assert_eq!(s.adjust(1, 10, Pocket::Bank, -101).unwrap(), Err(EcoError::NotEnough(100)));
        assert_eq!(s.adjust(1, 10, Pocket::Bank, -100).unwrap(), Ok(Balance { wallet: 25, bank: 0 }));
        assert_eq!(s.adjust(1, 10, Pocket::Wallet, MAX_BALANCE).unwrap(), Err(EcoError::TooLarge));
    }
}
