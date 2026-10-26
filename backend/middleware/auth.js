const isAuthenticated = (req, res, next) => {
  if (req.session && req.session.userId) {
    return next();
  }
  res.status(401).send({ message: 'Non autorisé. Veuillez vous connecter.' });
};

module.exports = { isAuthenticated };
