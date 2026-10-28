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
    /// /eco daily was already claimed today
    DailyClaimed,
}

/// The daily payout range for a role. `role_id` 0 is the default for everyone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DailyRate {
    pub role_id: u64,
    pub min: i64,
    pub max: i64,
}

/// Payout range when a server hasn't set a default
pub const DEFAULT_DAILY: DailyRate = DailyRate { role_id: 0, min: 100, max: 5000 };

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
            );
            CREATE TABLE IF NOT EXISTS daily_rates (
                guild_id INTEGER NOT NULL,
                -- 0 for the default rate
                role_id INTEGER NOT NULL,
                min_amount INTEGER NOT NULL,
                max_amount INTEGER NOT NULL,
                PRIMARY KEY (guild_id, role_id)
            );
            -- Bets taken from wallets for games still being played, refunded if the bot stops
            CREATE TABLE IF NOT EXISTS held (
                guild_id INTEGER NOT NULL,
                user_id INTEGER NOT NULL,
                amount INTEGER NOT NULL,
                PRIMARY KEY (guild_id, user_id)
            );",
        )?;

        // Databases created before /eco daily existed
        let has_daily: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('accounts') WHERE name = 'last_daily'",
            [],
            |row| row.get(0),
        )?;
        if !has_daily {
            // Day number (days since the Unix epoch, UTC) of the last /eco daily
            conn.execute_batch("ALTER TABLE accounts ADD COLUMN last_daily INTEGER")?;
        }
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

    /// Pays the daily reward into the wallet if it hasn't been claimed on day `today`.
    pub fn claim_daily(&mut self, guild_id: u64, user_id: u64, amount: i64, today: i64) -> EcoResult<Balance> {
        let tx = self.conn.transaction()?;
        let Some(mut balance) = balance_in(&tx, guild_id, user_id)? else { return Ok(Err(EcoError::NoAccount)) };

        let last_daily: Option<i64> =
            tx.query_row("SELECT last_daily FROM accounts WHERE guild_id = ? AND user_id = ?", [guild_id, user_id], |row| row.get(0))?;
        if last_daily.is_some_and(|day| day >= today) {
            return Ok(Err(EcoError::DailyClaimed));
        }

        balance.wallet = (balance.wallet + amount).min(MAX_BALANCE);
        set_balance(&tx, guild_id, user_id, balance)?;
        tx.execute("UPDATE accounts SET last_daily = ? WHERE guild_id = ? AND user_id = ?", params![today, guild_id, user_id])?;
        tx.commit()?;
        Ok(Ok(balance))
    }

    pub fn daily_rates(&self, guild_id: u64) -> rusqlite::Result<Vec<DailyRate>> {
        let mut stmt = self.conn.prepare("SELECT role_id, min_amount, max_amount FROM daily_rates WHERE guild_id = ? ORDER BY role_id")?;
        let rows = stmt.query_map([guild_id], |row| Ok(DailyRate { role_id: row.get(0)?, min: row.get(1)?, max: row.get(2)? }))?;
        rows.collect()
    }

    pub fn set_daily_rate(&self, guild_id: u64, rate: DailyRate) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO daily_rates (guild_id, role_id, min_amount, max_amount) VALUES (?, ?, ?, ?)",
            params![guild_id, rate.role_id, rate.min, rate.max],
        )?;
        Ok(())
    }

    /// Returns false if the role had no rate set.
    pub fn remove_daily_rate(&self, guild_id: u64, role_id: u64) -> rusqlite::Result<bool> {
        let removed = self.conn.execute("DELETE FROM daily_rates WHERE guild_id = ? AND role_id = ?", [guild_id, role_id])?;
        Ok(removed > 0)
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

    /// Takes a bet from the wallet and holds it until `settle` or `refund_held`.
    pub fn hold(&mut self, guild_id: u64, user_id: u64, amount: i64) -> EcoResult<Balance> {
        let tx = self.conn.transaction()?;
        let Some(mut balance) = balance_in(&tx, guild_id, user_id)? else { return Ok(Err(EcoError::NoAccount)) };
        if balance.wallet < amount {
            return Ok(Err(EcoError::NotEnough(balance.wallet)));
        }
        balance.wallet -= amount;
        set_balance(&tx, guild_id, user_id, balance)?;
        tx.execute(
            "INSERT INTO held (guild_id, user_id, amount) VALUES (?, ?, ?)
             ON CONFLICT (guild_id, user_id) DO UPDATE SET amount = amount + excluded.amount",
            params![guild_id, user_id, amount],
        )?;
        tx.commit()?;
        Ok(Ok(balance))
    }

    /// Releases the member's held bet and pays `payout` (which may be 0) into their wallet,
    /// in one go so a crash can't both refund and pay them. A payout that would overflow
    /// the wallet is dropped with `TooLarge`.
    pub fn settle(&mut self, guild_id: u64, user_id: u64, payout: i64) -> EcoResult<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM held WHERE guild_id = ? AND user_id = ?", [guild_id, user_id])?;
        let result = match balance_in(&tx, guild_id, user_id)? {
            None => Err(EcoError::NoAccount),
            Some(balance) if balance.wallet + payout > MAX_BALANCE => Err(EcoError::TooLarge),
            Some(mut balance) => {
                balance.wallet += payout;
                set_balance(&tx, guild_id, user_id, balance)?;
                Ok(())
            }
        };
        tx.commit()?;
        Ok(result)
    }

    /// Returns held bets to their wallets, in one guild or (on startup) everywhere.
    /// Returns how many were refunded.
    pub fn refund_held(&mut self, guild_id: Option<u64>) -> rusqlite::Result<usize> {
        let tx = self.conn.transaction()?;
        let refunds = tx.execute(
            "UPDATE accounts SET wallet = MIN(wallet + (
                 SELECT amount FROM held WHERE held.guild_id = accounts.guild_id AND held.user_id = accounts.user_id
             ), ?1)
             WHERE (accounts.guild_id, accounts.user_id) IN (SELECT guild_id, user_id FROM held WHERE ?2 IS NULL OR guild_id = ?2)",
            params![MAX_BALANCE, guild_id],
        )?;
        tx.execute("DELETE FROM held WHERE ?1 IS NULL OR guild_id = ?1", params![guild_id])?;
        tx.commit()?;
        Ok(refunds)
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
    fn daily_can_be_claimed_once_per_day() {
        let mut s = store();
        assert_eq!(s.claim_daily(1, 10, 500, 20000).unwrap(), Err(EcoError::NoAccount));
        s.join(1, 10).unwrap();
        assert_eq!(s.claim_daily(1, 10, 500, 20000).unwrap(), Ok(Balance { wallet: 500, bank: 100 }));
        assert_eq!(s.claim_daily(1, 10, 500, 20000).unwrap(), Err(EcoError::DailyClaimed));
        assert_eq!(s.claim_daily(1, 10, 250, 20001).unwrap().unwrap().wallet, 750);
    }

    #[test]
    fn daily_rates_per_role() {
        let s = store();
        assert!(s.daily_rates(1).unwrap().is_empty());
        s.set_daily_rate(1, DailyRate { role_id: 0, min: 10, max: 20 }).unwrap();
        s.set_daily_rate(1, DailyRate { role_id: 55, min: 100, max: 200 }).unwrap();
        s.set_daily_rate(1, DailyRate { role_id: 55, min: 300, max: 400 }).unwrap();
        assert_eq!(
            s.daily_rates(1).unwrap(),
            vec![DailyRate { role_id: 0, min: 10, max: 20 }, DailyRate { role_id: 55, min: 300, max: 400 }]
        );
        assert!(s.remove_daily_rate(1, 55).unwrap());
        assert!(!s.remove_daily_rate(1, 55).unwrap());
    }

    #[test]
    fn adds_daily_column_to_old_databases() {
        let path = std::env::temp_dir().join(format!("economy-test-{}.db", std::process::id()));
        let path = path.to_str().unwrap();
        Connection::open(path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE accounts (guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, wallet INTEGER NOT NULL DEFAULT 0,
                 bank INTEGER NOT NULL DEFAULT 0, last_work INTEGER, PRIMARY KEY (guild_id, user_id));
                 INSERT INTO accounts (guild_id, user_id, wallet, bank) VALUES (1, 10, 5, 100);",
            )
            .unwrap();
        let mut s = Store::open_path(path).unwrap();
        assert_eq!(s.claim_daily(1, 10, 1, 1).unwrap(), Ok(Balance { wallet: 6, bank: 100 }));
        std::fs::remove_file(path).ok();
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

    #[test]
    fn held_bets_are_settled_or_refunded() {
        let mut s = store();
        for user in [10, 11] {
            s.join(1, user).unwrap();
            s.adjust(1, user, Pocket::Wallet, 100).unwrap().unwrap();
        }
        assert_eq!(s.hold(1, 10, 500).unwrap(), Err(EcoError::NotEnough(100)));
        assert_eq!(s.hold(1, 10, 40).unwrap().unwrap().wallet, 60);
        assert_eq!(s.hold(1, 11, 40).unwrap().unwrap().wallet, 60);

        // Settled bets aren't refunded again
        s.settle(1, 10, 52).unwrap().unwrap();
        assert_eq!(s.refund_held(None).unwrap(), 1);
        assert_eq!(s.balance(1, 10).unwrap().unwrap().wallet, 112);
        assert_eq!(s.balance(1, 11).unwrap().unwrap().wallet, 100);
        assert_eq!(s.refund_held(None).unwrap(), 0);
    }
}
